use clap::Parser;
use swayipc::{Connection, EventType, Node};
use swayipc::{Event, WindowChange};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

fn matches_any(conn: &mut Connection, pid: u32, criteria: &[String], mark: &str) -> bool {
    criteria.iter().any(|c| {
        conn.run_command(format!("[pid={} {}] mark --add {}", pid, c, mark))
            // what the fuck
            .ok()
            .and_then(|outcomes| outcomes.into_iter().next())
            .and_then(|o| o.ok())
            .is_some()
    })
}

fn is_terminal(conn: &mut Connection, pid: u32, criteria: &[String]) -> bool {
    matches_any(conn, pid, criteria, "_is_terminal")
}

fn is_noswallow(conn: &mut Connection, pid: u32, criteria: &[String]) -> bool {
    matches_any(conn, pid, criteria, "_is_noswallow")
}

fn find_parent_view_pid(tree: &Node, child_pid: u32, sys: &mut System) -> Option<u32> {
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut current_pid = child_pid;
    loop {
        // next parent
        current_pid = sys.process(Pid::from_u32(current_pid))?.parent()?.as_u32();

        // check if it's a sway view node (only views have PID set)
        let parent = tree.find_as_ref(|n| n.pid == Some(current_pid as i32));
        if parent.is_some() {
            return Some(current_pid);
        }
    }
}

fn swallow(
    conn: &mut Connection,
    sys: &mut System,
    node: &Node,
    args: &Args,
) -> Result<(), String> {
    let tree = conn.get_tree().map_err(|_| "get_tree() failed")?;

    let pid = node.pid.ok_or("Missing PID")? as u32;
    if is_terminal(conn, pid, &args.terminal) || is_noswallow(conn, pid, &args.noswallow) {
        return Ok(());
    }

    let parent_pid = find_parent_view_pid(&tree, pid, sys).ok_or("No parent")?;
    if !is_terminal(conn, parent_pid, &args.terminal) {
        return Ok(());
    }

    conn.run_command(format!(
        "[pid={}] move scratchpad, mark --add _swallow_{}",
        parent_pid, pid
    ))
    .unwrap();

    Ok(())
}

fn unswallow(conn: &mut Connection, node: &Node) -> Result<(), String> {
    let pid = node.pid.ok_or("Missing PID")? as u32;

    conn.run_command(format!(
        "[con_mark=_swallow_{}] focus, floating disable, unmark _swallow_{}",
        pid, pid
    ))
    .unwrap();

    Ok(())
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Terminal window matcher in Sway criteria format (e.g. 'app_id="foot"').
    /// May be specified multiple times.
    #[arg(short, long, value_name = "CRITERIA", required = true, num_args = 1..)]
    terminal: Vec<String>,

    /// Window that should never be swallowed, in Sway criteria format (e.g. 'app_id="firefox"').
    /// May be specified multiple times.
    #[arg(short, long, value_name = "CRITERIA", num_args = 1..)]
    noswallow: Vec<String>,
}

fn main() -> Result<(), std::io::Error> {
    let args = Args::parse();

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );

    let mut conn = Connection::new().unwrap();
    for event in Connection::new()
        .unwrap()
        .subscribe([EventType::Window])
        .unwrap()
    {
        match event.unwrap() {
            Event::Window(e) => match e.change {
                WindowChange::New => {
                    if let Err(err) = swallow(&mut conn, &mut sys, &e.container, &args) {
                        eprintln!("err: {}", err);
                    }
                }
                WindowChange::Close => {
                    if let Err(err) = unswallow(&mut conn, &e.container) {
                        eprintln!("err: {}", err);
                    }
                }
                _ => {}
            },
            _ => unreachable!(),
        }
    }

    Ok(())
}
