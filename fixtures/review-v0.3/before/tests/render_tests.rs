use review_fixture::{render, render_page};

#[test]
fn render_page_uses_render() {
    let rendered = render("home");
    let page = render_page("home");
    assert_eq!(rendered, "legacy:home");
    assert_eq!(page, "legacy:home");
}
