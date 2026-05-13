use std::collections::HashMap;

use swayipc::{Connection, EventType, Node};
use swayipc::{Event, WindowChange};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind, System};

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
    pid_map: &mut HashMap<u32, u32>,
    sys: &mut System,
    node: &Node,
) -> Result<(), String> {
    let tree = conn.get_tree().map_err(|_| "get_tree() failed")?;

    let pid = node.pid.ok_or("Missing PID")? as u32;
    let parent_pid = find_parent_view_pid(&tree, pid, sys).ok_or("No parent")?;

    // hide the parent node
    conn.run_command(format!("[pid={}] focus; move scratchpad", parent_pid))
        .unwrap();

    pid_map.insert(pid, parent_pid);

    Ok(())
}

fn unswallow(
    conn: &mut Connection,
    pid_map: &mut HashMap<u32, u32>,
    node: &Node,
) -> Result<(), String> {
    let pid = node.pid.ok_or("Missing PID")? as u32;

    if let Some(parent_pid) = pid_map.remove(&pid) {
        // return the parent node
        conn.run_command(format!("[pid={}] focus; floating disable", parent_pid))
            .unwrap();
    }

    Ok(())
}

fn main() -> Result<(), std::io::Error> {
    let mut pid_map: HashMap<u32, u32> = HashMap::new();

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
                    if let Err(err) = swallow(&mut conn, &mut pid_map, &mut sys, &e.container) {
                        eprintln!("err: {}", err);
                    }
                }
                WindowChange::Close => {
                    if let Err(err) = unswallow(&mut conn, &mut pid_map, &e.container) {
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
