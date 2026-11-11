use std::collections::BTreeSet;

pub fn groups(session: i32) -> Vec<i32> {
    let mut groups: BTreeSet<i32> = pids()
        .into_iter()
        .filter(|&pid| unsafe { libc::getsid(pid) } == session)
        .map(|pid| unsafe { libc::getpgid(pid) })
        .filter(|&group| group > 0)
        .collect();
    groups.insert(session);
    groups.into_iter().collect()
}

pub fn alive(session: i32) -> bool {
    pids().into_iter().any(|pid| unsafe { libc::getsid(pid) } == session)
}

#[cfg(target_os = "macos")]
fn pids() -> Vec<i32> {
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return vec![];
    }
    let mut pids = vec![0i32; count as usize + 64];
    let size = (pids.len() * std::mem::size_of::<i32>()) as libc::c_int;
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(n.max(0) as usize);
    pids.retain(|&pid| pid > 0);
    pids
}

#[cfg(not(target_os = "macos"))]
fn pids() -> Vec<i32> {
    std::fs::read_dir("/proc")
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_has_its_own_group_and_the_ones_its_processes_moved_to() {
        let mine = unsafe { libc::getsid(0) };
        let found = groups(mine);
        assert!(found.contains(&mine) && found.contains(&unsafe { libc::getpgid(0) }), "{found:?}");
        assert_eq!(groups(i32::MAX - 1), [i32::MAX - 1]);
    }

    #[test]
    fn a_session_is_alive_while_a_process_is_in_it() {
        assert!(alive(unsafe { libc::getsid(0) }));
        assert!(!alive(i32::MAX - 1));
    }
}
