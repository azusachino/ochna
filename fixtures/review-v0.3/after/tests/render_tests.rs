use review_fixture::{render, render_page};

#[test]
fn render_page_uses_render() {
    let rendered = render("home");
    let page = render_page("home");
    assert_eq!(rendered, "rendered:home");
    assert_eq!(page, "rendered:home");
}
