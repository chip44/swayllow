use clap::Parser;
use swayipc::{Connection, EventType, Node};
use swayipc::{Event, WindowChange};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

trait OptionExt<T> {
    fn ok_or_sway(self, msg: &str) -> Result<T, swayipc::Error>;
}

impl<T> OptionExt<T> for Option<T> {
    /// small helper to convert `Result<T, Error>` to `Result<T, swayipc::Error>`
    /// because sometimes rust sucks
    fn ok_or_sway(self, msg: &str) -> Result<T, swayipc::Error> {
        self.ok_or_else(|| swayipc::Error::Io(std::io::Error::other(msg)))
    }
}

fn matches_any(conn: &mut Connection, pid: u32, criteria: &[String], mark: &str) -> bool {
    criteria.iter().any(|c| {
        // basically a no-op, we just need to check if any window matches the criteria
        conn.run_command(format!("[pid={pid} {c}] mark --add {mark}"))
            // what the fuck
            .ok()
            .and_then(|outcomes| outcomes.into_iter().next())
            .and_then(Result::ok)
            .is_some()
    })
}

fn is_terminal(conn: &mut Connection, pid: u32, criteria: &[String]) -> bool {
    matches_any(conn, pid, criteria, "_is_terminal")
}

fn is_noswallow(conn: &mut Connection, pid: u32, criteria: &[String]) -> bool {
    matches_any(conn, pid, criteria, "_is_noswallow")
}

/// find the closest parent process that has a valid view node in the tree,
/// since not all processes appear as views (e.g. shell processes).
fn find_parent_view_pid(tree: &Node, child_pid: u32, sys: &mut System) -> Option<u32> {
    sys.refresh_processes(ProcessesToUpdate::All, true);

    let mut current_pid = child_pid;
    loop {
        // next parent
        current_pid = sys.process(Pid::from_u32(current_pid))?.parent()?.as_u32();

        // check if it's a sway view node (only views have PID set)
        let parent = tree.find_as_ref(|n| n.pid == Some(current_pid.cast_signed()));
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
) -> Result<(), swayipc::Error> {
    let pid = node.pid.ok_or_sway("Missing PID")?.cast_unsigned();
    if is_terminal(conn, pid, &args.terminal) || is_noswallow(conn, pid, &args.noswallow) {
        return Ok(());
    }

    let tree = conn.get_tree()?;
    let parent_pid = find_parent_view_pid(&tree, pid, sys).ok_or_sway("No parent")?;
    if !is_terminal(conn, parent_pid, &args.terminal) {
        return Ok(());
    }

    // move the parent to the scratchpad and mark it with a unique mark to be
    // able to restore it later
    conn.run_command(format!(
        "[pid={parent_pid}] move scratchpad, mark --add _swallow_{pid};
         split none"
    ))?;

    Ok(())
}

fn unswallow(conn: &mut Connection, node: &Node) -> Result<(), swayipc::Error> {
    let pid = node.pid.ok_or_sway("Missing PID")?;

    // a program can have multiple windows sharing the same PID (e.g. settings
    // dialogs). only unswallow once every window for this PID is actually gone,
    // otherwise closing just the dialog would return the terminal prematurely.
    let tree = conn.get_tree()?;
    if tree.find_as_ref(|n| n.pid == Some(pid)).is_some() {
        return Ok(());
    }

    conn.run_command(format!(
        "[con_mark=_swallow_{pid}] focus, floating disable, unmark _swallow_{pid}"
    ))?;

    Ok(())
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Terminal window matcher in Sway criteria format (e.g. `app_id="foot"`).
    /// May be specified multiple times.
    #[arg(short, long, value_name = "CRITERIA", required = true, num_args = 1..)]
    terminal: Vec<String>,

    /// Window that should never be swallowed, in Sway criteria format (e.g. `app_id="firefox"`).
    /// May be specified multiple times.
    #[arg(short, long, value_name = "CRITERIA", num_args = 1..)]
    noswallow: Vec<String>,
}

fn main() -> Result<(), swayipc::Error> {
    let args = Args::parse();

    let mut sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(ProcessRefreshKind::everything()),
    );

    let mut conn = Connection::new()?;
    for event in Connection::new()?.subscribe([EventType::Window])? {
        match event? {
            Event::Window(e) => match e.change {
                WindowChange::New => {
                    if let Err(err) = swallow(&mut conn, &mut sys, &e.container, &args) {
                        eprintln!("err: {err}");
                    }
                }
                WindowChange::Close => {
                    if let Err(err) = unswallow(&mut conn, &e.container) {
                        eprintln!("err: {err}");
                    }
                }
                _ => {}
            },
            _ => unreachable!(),
        }
    }

    Ok(())
}
