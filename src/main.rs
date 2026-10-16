use clap::Parser;
use swayipc::{Connection, Event, EventType, Node, NodeLayout, NodeType, WindowChange};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// mark prefix to put on a terminal (e.g. `_swallow_1234`).
const MARK_PREFIX: &str = "_swallow_";

/// transform `Option<T>` to `Result<T, swayipc::Error>`
fn sway_err<T>(opt: Option<T>, msg: &str) -> Result<T, swayipc::Error> {
    opt.ok_or_else(|| swayipc::Error::Io(std::io::Error::other(msg)))
}

/// run a batched command and check if any of them failed
fn run(conn: &mut Connection, cmd: impl AsRef<str>) -> Result<(), swayipc::Error> {
    for outcome in conn.run_command(cmd)? {
        outcome?;
    }
    Ok(())
}

fn matches_any(conn: &mut Connection, pid: u32, criteria: &[String], mark: &str) -> bool {
    criteria.iter().any(|c| {
        // basically a no-op, we just need to check if any window matches the criteria
        run(conn, format!("[pid={pid} {c}] mark --add {mark};")).is_ok()
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
fn find_parent_view<'a>(tree: &'a Node, child_pid: u32, sys: &mut System) -> Option<&'a Node> {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().without_tasks(),
    );

    let mut current_pid = child_pid;
    loop {
        // next parent
        current_pid = sys.process(Pid::from_u32(current_pid))?.parent()?.as_u32();

        // check if it's a sway view node (only views have PID set)
        let parent = tree.find_as_ref(|n| n.pid == Some(current_pid.cast_signed()));
        if parent.is_some() {
            return parent;
        }
    }
}

/// find the container directly above the node with the given id
fn find_container_of(root: &Node, id: i64) -> Option<&Node> {
    root.nodes
        .iter()
        .chain(&root.floating_nodes)
        .find_map(|node| {
            if node.id == id {
                Some(root)
            } else {
                find_container_of(node, id)
            }
        })
}

/// if the node is the only tab left in a tabbed container, return the command that gets rid of that container again.
fn unwrap_cmd(tree: &Node, id: i64) -> Option<String> {
    let container = find_container_of(tree, id)?;
    (container.node_type == NodeType::Con
        && container.layout == NodeLayout::Tabbed
        && container.nodes.len() == 1)
        .then(|| format!("[con_id={id}] split none;"))
}

/// the tabbed container created by `swallow`
fn is_wrapper(n: &Node) -> bool {
    n.node_type == NodeType::Con
        && n.layout == NodeLayout::Tabbed
        && n.nodes
            .iter()
            .any(|c| c.marks.iter().any(|m| m.starts_with(MARK_PREFIX)))
}

/// if a new window opens next to a swallowed window (inside a tabbed container), it would end up as one more tab in there.
/// move it out again, to the spot right after the wrapper, which is where it would have gone without swallowing.
fn evict(conn: &mut Connection, node: &Node) -> Result<(), swayipc::Error> {
    let tree = conn.get_tree()?;
    let Some(wrapper) = find_container_of(&tree, node.id).filter(|w| is_wrapper(w)) else {
        return Ok(());
    };
    let Some(outer) = find_container_of(&tree, wrapper.id) else {
        return Ok(());
    };
    let Some(idx) = wrapper.nodes.iter().position(|n| n.id == node.id) else {
        return Ok(());
    };

    // moving along the tabs only swaps places with the next tab, so it takes one move per tab behind the window to get to the end, plus one to leave.
    // moving across the tabs leaves right away, so go along the axis of the container around the wrapper, which decides where the window lands.
    let (dir, steps) = match outer.layout {
        NodeLayout::SplitH | NodeLayout::Tabbed => ("right", wrapper.nodes.len() - idx),
        NodeLayout::SplitV | NodeLayout::Stacked => ("down", 1),
        _ => return Ok(()),
    };
    let cmd = vec![format!("[con_id={}] move {dir}", node.id); steps].join("; ");
    run(conn, cmd)
}

fn swallow(
    conn: &mut Connection,
    sys: &mut System,
    node: &Node,
    args: &Args,
) -> Result<(), swayipc::Error> {
    let pid = sway_err(node.pid, "missing pid")?.cast_unsigned();

    if is_terminal(conn, pid, &args.terminal) || is_noswallow(conn, pid, &args.noswallow) {
        return Ok(());
    }

    let tree = conn.get_tree()?;
    // the window may already be gone again
    let child = sway_err(tree.find_as_ref(|n| n.id == node.id), "window is gone")?;
    let parent = sway_err(find_parent_view(&tree, pid, sys), "no parent")?;
    let parent_pid = sway_err(parent.pid, "missing parent pid")?.cast_unsigned();
    if !is_terminal(conn, parent_pid, &args.terminal) {
        return Ok(());
    }

    // floating windows (like dialogs) can't share a tab
    let floating = |n: &Node| n.node_type == NodeType::FloatingCon;
    if floating(child)
        || floating(parent)
        || parent.marks.iter().any(|m| m.starts_with(MARK_PREFIX))
    {
        return Ok(());
    }

    let (p, c) = (parent.id, child.id);
    let mark = format!("{MARK_PREFIX}{pid}");
    let cmd = [
        // mark the terminal so it can be used and found later
        format!("[con_id={p}] mark --add {mark}"),
        // make sure the new window is the terminal's sibling (it usually already is, but not if focus changed before it appeared).
        // this is required for the next cmd, since `split` on a single window only changes the layout of its parent instead of creating a new container.
        format!("[con_id={c}] move container to mark {mark}"),
        // wrap the terminal in a new container of its own, which takes over the terminal's slot
        format!("[con_id={p}] split v"),
        // move the new window into that container, next to the terminal
        format!("[con_id={c}] move container to mark {mark}"),
        // make that container tabbed
        format!("[con_id={c}] layout tabbed"),
        // show the new window
        format!("[con_id={c}] focus"),
    ]
    .join("; ");
    run(conn, cmd)
}

fn unswallow(conn: &mut Connection, node: &Node) -> Result<(), swayipc::Error> {
    let pid = sway_err(node.pid, "missing pid")?;
    let tree = conn.get_tree()?;

    // terminal was closed while the child is still there, just remove the tabbed container
    let child_pid = node
        .marks
        .iter()
        .find_map(|m| m.strip_prefix(MARK_PREFIX)?.parse::<i32>().ok());
    if let Some(child_pid) = child_pid {
        let child = tree.find_as_ref(|n| n.pid == Some(child_pid));
        if let Some(cmd) = child.and_then(|c| unwrap_cmd(&tree, c.id)) {
            run(conn, cmd)?;
        }
        return Ok(());
    }

    // a program can have multiple windows sharing the same PID (e.g. settings
    // dialogs). only unswallow once every window for this PID is actually gone,
    // otherwise closing just the dialog would return the terminal prematurely.
    if tree.find_as_ref(|n| n.pid == Some(pid)).is_some() {
        return Ok(());
    }

    let mark = format!("{MARK_PREFIX}{pid}");
    let Some(parent) = tree.find_as_ref(|n| n.marks.contains(&mark)) else {
        return Ok(());
    };

    // remove the tabbed container and the mark from the terminal
    let unwrap = unwrap_cmd(&tree, parent.id).unwrap_or_default();
    let cmd = format!("{unwrap} [con_id={}] unmark {mark}", parent.id);
    run(conn, cmd)
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

    let mut sys = System::new();

    let mut conn = Connection::new()?;
    for event in Connection::new()?.subscribe([EventType::Window])? {
        match event? {
            Event::Window(e) => match e.change {
                WindowChange::New => {
                    if let Err(err) = evict(&mut conn, &e.container) {
                        eprintln!("err: {err}");
                    }
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
