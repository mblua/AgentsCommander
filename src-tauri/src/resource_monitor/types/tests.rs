use super::*;

pub(crate) fn observed_tree_for_test(
    root: ProcessIdentity,
    alive: bool,
    exe_name: &str,
) -> ObservedProcessTree {
    ObservedProcessTree {
        processes: if alive {
            vec![ObservedProcess {
                identity: root,
                parent_pid: None,
                parent_identity: None,
                exe_name: exe_name.into(),
                depth: 0,
                private_bytes: None,
                working_set_bytes: None,
                cpu_percent: None,
                kill_allowed: true,
            }]
        } else {
            Vec::new()
        },
        errors: Vec::new(),
    }
}
