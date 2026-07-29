use review_fixture::{render, render_page};

#[test]
fn render_page_uses_render() {
    assert_eq!(render("home"), "legacy:home");
    assert_eq!(render_page("home"), "legacy:home");
}
