use super::*;

#[test]
fn stub_for_child_is_none() {
    assert!(JobObject::for_child(1234).is_none());
}
