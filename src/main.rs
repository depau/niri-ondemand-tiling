use clap::{Parser, Subcommand, ValueEnum};
use niri_ipc::{Action, Request, Response, SizeChange};
use std::collections::HashMap;

#[derive(Parser)]
#[command(name = "niri-ondemand-tiling", about = "On-demand tiling for Niri")]
struct Cli {
    /// Log level (error, warn, info, debug, trace)
    #[arg(global = true, long, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Tile windows on a workspace
    Tile {
        /// Resize mode: proportional (default) or equal
        #[arg(short = 'm', long, default_value = "proportional")]
        mode: Mode,

        /// Workspace: omit for focused, pass a number (index), a name, or '#N' for an id
        #[arg(short = 'w', long)]
        workspace: Option<String>,
    },
}

#[derive(ValueEnum, Clone, Debug)]
enum Mode {
    /// Resize all columns to equal width (1/N each)
    Equal,
    /// Shrink columns proportionally to their current relative sizes
    Proportional,
}

fn ipc<T, F: FnOnce(Response) -> Option<T>>(req: Request, extract: F) -> Result<T, String> {
    let mut sock = niri_ipc::socket::Socket::connect().map_err(|e| e.to_string())?;
    let reply = sock.send(req).map_err(|e| e.to_string())?;
    let resp = reply.map_err(|e| e)?;
    extract(resp).ok_or_else(|| "unexpected response type".to_string())
}

fn send_actions(actions: Vec<Action>) -> Result<(), String> {
    for action in actions {
        ipc(Request::Action(action), |r| {
            if matches!(r, Response::Handled) { Some(()) } else { None }
        })?;
    }
    Ok(())
}

fn run(workspace_arg: Option<&str>, mode: Mode) -> Result<(), String> {
    // Resolve workspace
    let workspaces = ipc(Request::Workspaces, |r| {
        if let Response::Workspaces(ws) = r { Some(ws) } else { None }
    })?;

    let ws = match workspace_arg.map(parse_workspace_ref) {
        None => workspaces.iter().find(|w| w.is_focused),
        Some(WorkspaceRef::Index(idx)) => workspaces.iter().find(|w| w.idx == idx),
        Some(WorkspaceRef::Name(ref name)) => workspaces.iter().find(|w| w.name.as_deref() == Some(name.as_str())),
        Some(WorkspaceRef::Id(id)) => workspaces.iter().find(|w| w.id == id),
    }.ok_or("workspace not found")?;

    let ws_id = ws.id;
    let output_name = ws.output.clone();

    // Get output width
    let output_width = if let Some(name) = output_name {
        let outputs = ipc(Request::Outputs, |r| {
            if let Response::Outputs(m) = r { Some(m) } else { None }
        })?;
        outputs.get(&name)
            .and_then(|o| o.logical.as_ref())
            .map(|l| l.width as f64)
            .unwrap_or(1920.0)
    } else {
        1920.0
    };

    // Get windows on this workspace, grouped into columns
    let windows = ipc(Request::Windows, |r| {
        if let Response::Windows(ws) = r { Some(ws) } else { None }
    })?;

    let focused_win_id = windows.iter()
        .find(|w| w.workspace_id == Some(ws_id) && w.is_focused)
        .map(|w| w.id);

    // Build column list: col_idx -> (first_window_id, width, x)
    let mut col_map: HashMap<usize, (u64, f64, f64)> = HashMap::new();
    for win in windows.iter().filter(|w| {
        w.workspace_id == Some(ws_id) && !w.is_floating && w.layout.pos_in_scrolling_layout.is_some()
    }) {
        let (col_idx, _) = win.layout.pos_in_scrolling_layout.unwrap();
        col_map.entry(col_idx).or_insert_with(|| {
            let x = win.layout.tile_pos_in_workspace_view.map(|(x, _)| x).unwrap_or(0.0);
            (win.id, win.layout.tile_size.0, x)
        });
    }

    if col_map.is_empty() {
        log::info!("No tiling windows on workspace. Nothing to do.");
        return Ok(());
    }

    let mut cols: Vec<(usize, u64, f64, f64)> = col_map.into_iter()
        .map(|(idx, (win_id, width, x))| (idx, win_id, width, x))
        .collect();
    cols.sort_by_key(|c| c.0); // sort by column index

    let n = cols.len() as f64;
    log::info!("Tiling workspace {} ({} columns, mode={:?})", ws_id, cols.len(), mode);

    // Compute raw proportions (pre-normalisation)
    let raw_props: Vec<f64> = match mode {
        Mode::Equal => vec![100.0 / n; cols.len()],
        Mode::Proportional => {
            // Estimate left margin: x of leftmost column
            let left_x = cols.first().map(|c| c.3).unwrap_or(0.0);
            // Estimate right margin symmetrically
            let right_x = cols.last().map(|c| c.3 + c.2).unwrap_or(output_width);
            let right_margin = output_width - right_x;
            // If margins look symmetric (within 5px), use average; otherwise use left margin
            let margin = if (left_x - right_margin).abs() < 5.0 {
                (left_x + right_margin) / 2.0
            } else {
                left_x
            };
            let available = output_width - 2.0 * margin;
            let total_w: f64 = cols.iter().map(|c| c.2).sum();
            log::debug!("Proportional: output={:.0} margin={:.1} available={:.0} total_w={:.0}",
                output_width, margin, available, total_w);
            if total_w <= 0.0 {
                vec![100.0 / n; cols.len()]
            } else {
                cols.iter().map(|c| (c.2 / total_w) * 100.0).collect()
            }
        }
    };

    // Normalize so sum == 100.0
    let sum: f64 = raw_props.iter().sum();
    let props: Vec<f64> = raw_props.iter().map(|p| p / sum * 100.0).collect();

    // Build resize actions
    let resize_actions: Vec<Action> = cols.iter().zip(props.iter()).map(|(col, &prop)| {
        log::debug!("Column {} (win {}) -> {:.3}%", col.0, col.1, prop);
        Action::SetWindowWidth { id: Some(col.1), change: SizeChange::SetProportion(prop) }
    }).collect();

    send_actions(resize_actions)?;

    // Give Niri time to apply the resizes before snapping the viewport.
    // Without this pause the focus sequence lands before column widths are committed.
    if focused_win_id.is_some() {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let fid = focused_win_id.unwrap();
        log::debug!("Sending viewport alignment: FocusColumnLast, FocusColumnFirst, FocusWindow({})", fid);
        send_actions(vec![
            Action::FocusColumnLast {},
            Action::FocusColumnFirst {},
            Action::FocusWindow { id: fid },
        ])?;
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let level_filter = match cli.log_level.to_lowercase().as_str() {
        "error" => log::LevelFilter::Error,
        "warn"  => log::LevelFilter::Warn,
        "info"  => log::LevelFilter::Info,
        "debug" => log::LevelFilter::Debug,
        "trace" => log::LevelFilter::Trace,
        _       => log::LevelFilter::Info,
    };
    env_logger::Builder::new().filter_level(level_filter).init();

    let result = match cli.command {
        Commands::Tile { mode, workspace } => run(workspace.as_deref(), mode),
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// Workspace reference: number = index, #N = id, anything else = name
enum WorkspaceRef { Index(u8), Name(String), Id(u64) }

fn parse_workspace_ref(s: &str) -> WorkspaceRef {
    if let Some(rest) = s.strip_prefix('#') {
        if let Ok(id) = rest.parse::<u64>() { return WorkspaceRef::Id(id); }
    }
    if let Ok(idx) = s.parse::<u8>() { return WorkspaceRef::Index(idx); }
    WorkspaceRef::Name(s.to_string())
}
