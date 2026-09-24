//! 内嵌前端资源（include_str!，无外部依赖）。
pub const INDEX_HTML: &str = include_str!("assets/index.html");
pub const APP_JS: &str = include_str!("assets/app.js");
