use reexport_crate_fixture::Widget;

#[test]
fn uses_the_public_type() {
    let widget = Widget { name: "widget".to_string() };

    assert_eq!(widget.schema(), "widget");
}
