use super::*;

#[test]
fn every_layer_gets_an_id_no_other_layer_had() {
    let ids = [next_id(), next_id(), next_id()];
    assert!(ids[0] != 0 && ids[0] < ids[1] && ids[1] < ids[2], "{ids:?}");
}
