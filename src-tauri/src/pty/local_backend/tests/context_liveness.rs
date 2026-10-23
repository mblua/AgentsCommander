use super::{context_liveness_from_child_liveness, ChildLiveness, ContextSessionLiveness};

#[test]
fn maps_every_contained_child_liveness_state() {
    assert_eq!(
        context_liveness_from_child_liveness(&ChildLiveness::Alive),
        ContextSessionLiveness::Live
    );
    assert_eq!(
        context_liveness_from_child_liveness(&ChildLiveness::Unqueryable("denied".into())),
        ContextSessionLiveness::Unavailable
    );
    assert_eq!(
        context_liveness_from_child_liveness(&ChildLiveness::Exited {
            code: 0,
            success: true,
        }),
        ContextSessionLiveness::SessionOver
    );
    assert_eq!(
        context_liveness_from_child_liveness(&ChildLiveness::Gone),
        ContextSessionLiveness::SessionOver
    );
}
