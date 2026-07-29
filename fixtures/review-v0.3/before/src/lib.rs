pub fn legacy_render(input: &str) -> String {
    format!("legacy:{input}")
}

pub fn render(input: &str) -> String {
    legacy_render(input)
}

pub fn render_page(input: &str) -> String {
    render(input)
}
