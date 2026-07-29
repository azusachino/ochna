pub fn render(input: &str) -> String {
    format!("rendered:{input}")
}

pub fn render_page(input: &str) -> String {
    missing_renderer();
    render(input)
}
