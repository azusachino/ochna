//! Tree-sitter parsing and call-graph resolution, split by concern:
//! - [`common`] — shared AST helpers (signature/doc extraction, node lookup).
//! - [`interner`] — the symbol index and string interner.
//! - [`resolve`] — raw-call-site to call-edge resolution.
//! - `rust`/`go`/`c_like`/`zig`/`java` — per-language AST traversals.
//!
//! [`parse_code`] is the entry point: it parses a source file and dispatches to
//! the matching per-language traversal.

use crate::db::{Node, RawCall};
use std::error::Error;
use tree_sitter::Parser;

mod c_like;
mod common;
mod go;
mod interner;
mod java;
mod resolve;
mod rust;
mod zig;

pub use interner::{
    CandidateByFile, CandidateList, InternedString, StringInterner, SymbolCandidate, SymbolIndex,
    SymbolIndexBuilder, SymbolIx,
};
pub use resolve::{resolve_calls_global, resolve_calls_local};

use c_like::{traverse_c_like, CLikeContext};

/// Parse supported source and extract nodes (symbols) plus the raw,
/// unresolved call sites within them. Call sites are returned unresolved so the
/// caller can resolve them against the whole-project symbol index (see
/// [`resolve_calls_global`]) rather than only the symbols in this one file.
pub fn parse_code(
    file_path: &str,
    content: &str,
    language: &str,
) -> Result<(Vec<Node>, Vec<RawCall>), Box<dyn Error>> {
    let mut parser = Parser::new();
    let lang = language.to_lowercase();

    match lang.as_str() {
        "rust" | "rs" => parser.set_language(&tree_sitter_rust::LANGUAGE.into())?,
        "go" => parser.set_language(&tree_sitter_go::LANGUAGE.into())?,
        "java" => parser.set_language(&tree_sitter_java::LANGUAGE.into())?,
        "c" => parser.set_language(&tree_sitter_c::LANGUAGE.into())?,
        "cpp" | "c++" | "cc" | "cxx" => parser.set_language(&tree_sitter_cpp::LANGUAGE.into())?,
        "zig" => parser.set_language(&tree_sitter_zig::LANGUAGE.into())?,
        _ => return Err(format!("Unsupported language: {}", language).into()),
    }

    let tree = parser
        .parse(content, None)
        .ok_or("Failed to parse code content")?;

    let mut nodes = Vec::new();
    let mut raw_calls = Vec::new();

    if lang == "rust" || lang == "rs" {
        rust::traverse_rust(
            tree.root_node(),
            content,
            file_path,
            None,
            &mut nodes,
            &mut raw_calls,
            None,
        );
    } else if lang == "go" {
        let package_node = common::find_child_by_kind(tree.root_node(), "package_clause");
        let package_name = package_node.and_then(|n| {
            common::find_child_by_kind(n, "package_identifier")
                .or_else(|| common::find_child_by_kind(n, "identifier"))
                .map(|name_node| {
                    name_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
        });
        let imports = go::collect_go_imports(tree.root_node(), content);
        go::traverse_go(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
            package_name.as_deref(),
            &imports,
        );
    } else if lang == "java" {
        let package_node = common::find_child_by_kind(tree.root_node(), "package_declaration");
        let package_name = package_node.and_then(|n| {
            common::find_child_by_kind(n, "scoped_identifier")
                .or_else(|| common::find_child_by_kind(n, "identifier"))
                .map(|name_node| {
                    name_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
        });

        let mut imports = Vec::new();
        let mut cursor = tree.root_node().walk();
        for child in tree.root_node().children(&mut cursor) {
            if child.kind() == "import_declaration" {
                if let Some(imported_node) = child
                    .child_by_field_name("name")
                    .or_else(|| common::find_child_by_kind(child, "scoped_identifier"))
                    .or_else(|| common::find_child_by_kind(child, "identifier"))
                {
                    let imported_str = imported_node
                        .utf8_text(content.as_bytes())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !imported_str.is_empty() {
                        imports.push(imported_str);
                    }
                }
            }
        }

        java::traverse_java(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
            None,
            None,
            package_name.as_deref(),
            &imports,
            None,
        );
    } else if lang == "c" || lang == "cpp" || lang == "c++" || lang == "cc" || lang == "cxx" {
        traverse_c_like(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            CLikeContext {
                parent_qualified_name: None,
                parent_is_type: false,
            },
            None,
            None,
        );
    } else if lang == "zig" {
        zig::traverse_zig(
            tree.root_node(),
            content,
            file_path,
            &mut nodes,
            &mut raw_calls,
            None,
            None,
        );
    }

    Ok((nodes, raw_calls))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rust_code() {
        let rust_code = r#"
/// A simple Rust struct.
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Point implementation.
impl Point {
    /// Create a new point.
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }

    /// Retrieve sum of coordinates.
    pub fn sum(&self) -> i32 {
        self.helper()
    }

    fn helper(&self) -> i32 {
        self.x + self.y
    }
}

/// Enum for directions.
pub enum Direction {
    Up,
    Down,
}

/// Trait for displaying point information.
pub trait Printable {
    fn print(&self);
}

fn test_free_fn() {
    let p = Point::new(1, 2);
    p.sum();
}
"#;

        let (nodes, calls) = parse_code("src/point.rs", rust_code, "rust").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Let's assert nodes
        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/point.rs::Point");
        assert_eq!(
            struct_node.doc_comment.as_deref(),
            Some("/// A simple Rust struct.")
        );
        assert_eq!(struct_node.signature.as_deref(), Some("pub struct Point"));

        let enum_node = nodes
            .iter()
            .find(|n| n.name == "Direction" && n.kind == "enum")
            .unwrap();
        assert_eq!(enum_node.id, "src/point.rs::Direction");
        assert_eq!(
            enum_node.doc_comment.as_deref(),
            Some("/// Enum for directions.")
        );

        let trait_node = nodes
            .iter()
            .find(|n| n.name == "Printable" && n.kind == "trait")
            .unwrap();
        assert_eq!(trait_node.id, "src/point.rs::Printable");
        assert_eq!(
            trait_node.doc_comment.as_deref(),
            Some("/// Trait for displaying point information.")
        );

        // Methods within impl block
        let new_method = nodes
            .iter()
            .find(|n| n.name == "new" && n.kind == "method")
            .unwrap();
        assert_eq!(new_method.id, "src/point.rs::Point::new");
        assert_eq!(
            new_method.doc_comment.as_deref(),
            Some("/// Create a new point.")
        );
        assert_eq!(
            new_method.signature.as_deref(),
            Some("pub fn new(x: i32, y: i32) -> Self")
        );

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.rs::Point::sum");
        assert_eq!(
            sum_method.doc_comment.as_deref(),
            Some("/// Retrieve sum of coordinates.")
        );

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/point.rs::Point::helper");

        let free_fn = nodes
            .iter()
            .find(|n| n.name == "test_free_fn" && n.kind == "function")
            .unwrap();
        assert_eq!(free_fn.id, "src/point.rs::test_free_fn");

        // Edges: sum calls helper
        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        // Edges: test_free_fn calls Point::new (via new) and sum
        let edge_free_new = edges
            .iter()
            .find(|e| e.source_id == free_fn.id && e.target_id == new_method.id)
            .unwrap();
        assert_eq!(edge_free_new.kind, "calls");

        let edge_free_sum = edges
            .iter()
            .find(|e| e.source_id == free_fn.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_free_sum.kind, "calls");
    }

    #[test]
    fn test_resolve_calls_global_prefers_explicit_namespace() {
        let nodes = vec![
            Node {
                id: "src/shapes.rs::Point::new".to_string(),
                name: "new".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Point::new".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 1,
                end_line: 1,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
                is_test: false,
            },
            Node {
                id: "src/shapes.rs::Line::new".to_string(),
                name: "new".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Line::new".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 2,
                end_line: 2,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
                is_test: false,
            },
            Node {
                id: "src/shapes.rs::build".to_string(),
                name: "build".to_string(),
                kind: "function".to_string(),
                qualified_name: Some("build".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 3,
                end_line: 3,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
                is_test: false,
            },
        ];
        let calls = vec![RawCall::new(
            "src/shapes.rs::build".to_string(),
            "Line::new".to_string(),
            3,
            4,
        )];

        let edges = resolve_calls_local(&nodes, &calls);

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source_id, "src/shapes.rs::build");
        assert_eq!(edges[0].target_id, "src/shapes.rs::Line::new");
    }

    #[test]
    fn test_symbol_index_interns_ids_paths_and_namespaces() {
        let nodes = vec![
            Node {
                id: "src/shapes.rs::Point::new".to_string(),
                name: "new".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Point::new".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 1,
                end_line: 1,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
                is_test: false,
            },
            Node {
                id: "src/shapes.rs::Point::sum".to_string(),
                name: "sum".to_string(),
                kind: "method".to_string(),
                qualified_name: Some("Point::sum".to_string()),
                file_path: "src/shapes.rs".to_string(),
                start_line: 2,
                end_line: 2,
                start_column: 0,
                end_column: 0,
                signature: None,
                doc_comment: None,
                is_test: false,
            },
        ];

        let index = SymbolIndex::from_nodes(&nodes);

        assert_eq!(index.symbols.len(), 2);
        assert_ne!(index.symbols[0].id, index.symbols[1].id);
        assert_eq!(index.symbols[0].file_path, index.symbols[1].file_path);
        assert_eq!(index.symbols[0].namespace, index.symbols[1].namespace);
        assert_eq!(
            index.strings[index.symbols[0].file_path as usize],
            "src/shapes.rs"
        );
        assert_eq!(
            index.strings[index.symbols[0].namespace.unwrap() as usize],
            "Point"
        );
    }

    #[test]
    fn test_parse_go_code() {
        let go_code = r#"
package geometry

// Point represents a 2D point.
type Point struct {
	X, Y int
}

// Printer is an interface.
type Printer interface {
	Print()
}

// NewPoint creates a new point.
func NewPoint(x, y int) *Point {
	return &Point{X: x, Y: y}
}

// Distance calculates distance.
func (p *Point) Distance() float64 {
	p.helper()
	return 0.0
}

func (p *Point) helper() {
	// nested helper
}
"#;

        let (nodes, calls) = parse_code("geometry.go", go_code, "go").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Assert nodes
        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "geometry.go::Point");
        assert_eq!(
            struct_node.doc_comment.as_deref(),
            Some("// Point represents a 2D point.")
        );
        assert_eq!(struct_node.signature.as_deref(), Some("type Point"));

        let interface_node = nodes
            .iter()
            .find(|n| n.name == "Printer" && n.kind == "interface")
            .unwrap();
        assert_eq!(interface_node.id, "geometry.go::Printer");
        assert_eq!(
            interface_node.doc_comment.as_deref(),
            Some("// Printer is an interface.")
        );

        let func_node = nodes
            .iter()
            .find(|n| n.name == "NewPoint" && n.kind == "function")
            .unwrap();
        assert_eq!(func_node.id, "geometry.go::NewPoint");
        assert_eq!(
            func_node.doc_comment.as_deref(),
            Some("// NewPoint creates a new point.")
        );
        assert_eq!(
            func_node.signature.as_deref(),
            Some("func NewPoint(x, y int) *Point")
        );

        let method_node = nodes
            .iter()
            .find(|n| n.name == "Distance" && n.kind == "method")
            .unwrap();
        assert_eq!(method_node.id, "geometry.go::Point::Distance");
        assert_eq!(
            method_node.doc_comment.as_deref(),
            Some("// Distance calculates distance.")
        );
        assert_eq!(
            method_node.signature.as_deref(),
            Some("func (p *Point) Distance() float64")
        );

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "geometry.go::Point::helper");

        // Edges: Distance calls helper
        let edge_call = edges
            .iter()
            .find(|e| e.source_id == method_node.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_call.kind, "calls");
    }

    #[test]
    fn test_parse_java_code() {
        let java_code = r#"
/**
 * A sample Java class.
 */
public class App {
    private String name;

    /**
     * Constructor.
     */
    public App(String name) {
        this.name = name;
        init();
    }

    /**
     * Init method.
     */
    public void init() {
        // init
    }

    /**
     * Run method.
     */
    public void run() {
        System.out.println("Run");
    }

    /**
     * Main entry point.
     */
    public static void main(String[] args) {
        App app = new App("Demo");
        app.run();
    }
}
"#;

        let (nodes, calls) = parse_code("src/App.java", java_code, "java").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        // Find class node
        let class_node = nodes
            .iter()
            .find(|n| n.name == "App" && n.kind == "class")
            .unwrap();
        assert_eq!(class_node.id, "src/App.java::App");
        assert_eq!(
            class_node.doc_comment.as_deref(),
            Some("/**\n * A sample Java class.\n */")
        );
        assert_eq!(class_node.signature.as_deref(), Some("public class App"));

        // Find constructor node
        let constr_node = nodes
            .iter()
            .find(|n| n.name == "App" && n.kind == "constructor")
            .unwrap();
        assert_eq!(constr_node.id, "src/App.java::App::App");
        assert_eq!(
            constr_node.doc_comment.as_deref(),
            Some("/**\n     * Constructor.\n     */")
        );
        assert_eq!(
            constr_node.signature.as_deref(),
            Some("public App(String name)")
        );

        // Find init method
        let init_method = nodes
            .iter()
            .find(|n| n.name == "init" && n.kind == "method")
            .unwrap();
        assert_eq!(init_method.id, "src/App.java::App::init");
        assert_eq!(
            init_method.doc_comment.as_deref(),
            Some("/**\n     * Init method.\n     */")
        );
        assert_eq!(init_method.signature.as_deref(), Some("public void init()"));

        // Find run method
        let run_method = nodes
            .iter()
            .find(|n| n.name == "run" && n.kind == "method")
            .unwrap();
        assert_eq!(run_method.id, "src/App.java::App::run");

        // Find main method
        let main_method = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "method")
            .unwrap();
        assert_eq!(main_method.id, "src/App.java::App::main");

        // Check call edges
        // Constructor App calls init
        let edge_constr_init = edges
            .iter()
            .find(|e| e.source_id == constr_node.id && e.target_id == init_method.id)
            .unwrap();
        assert_eq!(edge_constr_init.kind, "calls");

        // main calls constructor (via new App)
        let edge_main_constr = edges
            .iter()
            .find(|e| e.source_id == main_method.id && e.target_id == constr_node.id)
            .unwrap();
        assert_eq!(edge_main_constr.kind, "calls");

        // main calls run method (via app.run)
        let edge_main_run = edges
            .iter()
            .find(|e| e.source_id == main_method.id && e.target_id == run_method.id)
            .unwrap();
        assert_eq!(edge_main_run.kind, "calls");
    }

    #[test]
    fn test_parse_spring_routes() {
        let java_code = r#"
@RestController
@RequestMapping(value = "/api/v2")
public class UserController {
    @GetMapping("/users/{id}")
    public User getUser(@PathVariable String id) {
        return null;
    }

    @PostMapping(path = {"/users", "/create"})
    public User createUser() {
        return null;
    }

    @RequestMapping(value = "/status", method = RequestMethod.POST)
    public String getStatus() {
        return "OK";
    }
}
"#;

        let (nodes, calls) = parse_code("src/UserController.java", java_code, "java").unwrap();

        let class_node = nodes
            .iter()
            .find(|n| n.name == "UserController" && n.kind == "class")
            .unwrap();
        assert_eq!(class_node.id, "src/UserController.java::UserController");

        let get_user = nodes
            .iter()
            .find(|n| n.name == "getUser" && n.kind == "method")
            .unwrap();
        let create_user = nodes
            .iter()
            .find(|n| n.name == "createUser" && n.kind == "method")
            .unwrap();
        let get_status = nodes
            .iter()
            .find(|n| n.name == "getStatus" && n.kind == "method")
            .unwrap();

        let route_get = nodes
            .iter()
            .find(|n| {
                n.id == "src/UserController.java::UserController::getUser::route::GET /api/v2/users/{id}"
            })
            .unwrap();
        assert_eq!(route_get.name, "GET /api/v2/users/{id}");
        assert_eq!(route_get.kind, "route");
        assert_eq!(
            route_get.qualified_name.as_deref(),
            Some("GET /api/v2/users/{id}")
        );

        let route_create1 = nodes
            .iter()
            .find(|n| {
                n.id == "src/UserController.java::UserController::createUser::route::POST /api/v2/users"
            })
            .unwrap();
        assert_eq!(route_create1.name, "POST /api/v2/users");

        let route_create2 = nodes
            .iter()
            .find(|n| {
                n.id == "src/UserController.java::UserController::createUser::route::POST /api/v2/create"
            })
            .unwrap();
        assert_eq!(route_create2.name, "POST /api/v2/create");

        let route_status = nodes
            .iter()
            .find(|n| {
                n.id == "src/UserController.java::UserController::getStatus::route::POST /api/v2/status"
            })
            .unwrap();
        assert_eq!(route_status.name, "POST /api/v2/status");

        let edges = resolve_calls_local(&nodes, &calls);

        let edge_get = edges.iter().find(|e| e.source_id == route_get.id).unwrap();
        assert_eq!(edge_get.target_id, get_user.id);
        assert_eq!(edge_get.kind, "calls");

        let edge_create1 = edges
            .iter()
            .find(|e| e.source_id == route_create1.id)
            .unwrap();
        assert_eq!(edge_create1.target_id, create_user.id);

        let edge_create2 = edges
            .iter()
            .find(|e| e.source_id == route_create2.id)
            .unwrap();
        assert_eq!(edge_create2.target_id, create_user.id);

        let edge_status = edges
            .iter()
            .find(|e| e.source_id == route_status.id)
            .unwrap();
        assert_eq!(edge_status.target_id, get_status.id);
    }

    #[test]
    fn test_parse_spring_routes_keeps_duplicate_paths_distinct() {
        let first = r#"
@RestController
@RequestMapping("/api")
public class FirstController {
    @RequestMapping("/status")
    public String status() {
        return "first";
    }
}
"#;
        let second = r#"
@RestController
@RequestMapping("/api")
public class SecondController {
    @RequestMapping("/status")
    public String status() {
        return "second";
    }
}
"#;

        let (mut nodes, mut calls) = parse_code("src/FirstController.java", first, "java").unwrap();
        let (second_nodes, second_calls) =
            parse_code("src/SecondController.java", second, "java").unwrap();
        nodes.extend(second_nodes);
        calls.extend(second_calls);

        let first_route = nodes
            .iter()
            .find(|n| {
                n.id == "src/FirstController.java::FirstController::status::route::ANY /api/status"
            })
            .unwrap();
        let second_route = nodes
            .iter()
            .find(|n| {
                n.id
                    == "src/SecondController.java::SecondController::status::route::ANY /api/status"
            })
            .unwrap();
        assert_ne!(first_route.id, second_route.id);
        assert_eq!(first_route.name, "ANY /api/status");
        assert_eq!(second_route.name, "ANY /api/status");

        let edges = resolve_calls_local(&nodes, &calls);
        let first_status = nodes
            .iter()
            .find(|n| n.id == "src/FirstController.java::FirstController::status")
            .unwrap();
        let second_status = nodes
            .iter()
            .find(|n| n.id == "src/SecondController.java::SecondController::status")
            .unwrap();
        assert!(edges
            .iter()
            .any(|e| e.source_id == first_route.id && e.target_id == first_status.id));
        assert!(edges
            .iter()
            .any(|e| e.source_id == second_route.id && e.target_id == second_status.id));
    }

    #[test]
    fn test_parse_c_code() {
        let c_code = r#"
// Point struct.
struct Point {
    int x;
    int y;
};

// Helper function.
int helper(int value) {
    return value + 1;
}

// Main entry point.
int main(void) {
    return helper(41);
}
"#;

        let (nodes, calls) = parse_code("src/main.c", c_code, "c").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/main.c::Point");
        assert_eq!(struct_node.doc_comment.as_deref(), Some("// Point struct."));

        let helper = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "function")
            .unwrap();
        assert_eq!(helper.id, "src/main.c::helper");
        assert_eq!(helper.doc_comment.as_deref(), Some("// Helper function."));

        let main = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "function")
            .unwrap();
        assert_eq!(main.id, "src/main.c::main");

        let edge_main_helper = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == helper.id)
            .unwrap();
        assert_eq!(edge_main_helper.kind, "calls");
    }

    #[test]
    fn test_parse_cpp_code() {
        let cpp_code = r#"
namespace demo {
// Point class.
class Point {
public:
    int sum() {
        return helper();
    }

    int helper() {
        return 42;
    }
};

int run() {
    Point point;
    return point.sum();
}
}
"#;

        let (nodes, calls) = parse_code("src/point.cpp", cpp_code, "cpp").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let namespace = nodes
            .iter()
            .find(|n| n.name == "demo" && n.kind == "namespace")
            .unwrap();
        assert_eq!(namespace.id, "src/point.cpp::demo");

        let class_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "class")
            .unwrap();
        assert_eq!(class_node.id, "src/point.cpp::demo::Point");
        assert_eq!(class_node.doc_comment.as_deref(), Some("// Point class."));

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.cpp::demo::Point::sum");

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/point.cpp::demo::Point::helper");

        let run = nodes
            .iter()
            .find(|n| n.name == "run" && n.kind == "function")
            .unwrap();
        assert_eq!(run.id, "src/point.cpp::demo::run");

        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        let edge_run_sum = edges
            .iter()
            .find(|e| e.source_id == run.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_run_sum.kind, "calls");
    }

    #[test]
    fn test_parse_zig_code() {
        let zig_code = r#"
/// Point type.
const Point = struct {
    x: i32,
    y: i32,

    /// Sum fields.
    fn sum(self: Point) i32 {
        return self.helper();
    }

    fn helper(self: Point) i32 {
        return self.x + self.y;
    }
};

fn makePoint() Point {
    return Point{ .x = 1, .y = 2 };
}

pub fn main() void {
    const point = makePoint();
    _ = point.sum();
}
"#;

        let (nodes, calls) = parse_code("src/main.zig", zig_code, "zig").unwrap();
        let edges = resolve_calls_local(&nodes, &calls);

        let struct_node = nodes
            .iter()
            .find(|n| n.name == "Point" && n.kind == "struct")
            .unwrap();
        assert_eq!(struct_node.id, "src/main.zig::Point");
        assert_eq!(struct_node.doc_comment.as_deref(), Some("/// Point type."));

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/main.zig::Point::sum");
        assert_eq!(sum_method.doc_comment.as_deref(), Some("/// Sum fields."));

        let helper_method = nodes
            .iter()
            .find(|n| n.name == "helper" && n.kind == "method")
            .unwrap();
        assert_eq!(helper_method.id, "src/main.zig::Point::helper");

        let make_point = nodes
            .iter()
            .find(|n| n.name == "makePoint" && n.kind == "function")
            .unwrap();
        assert_eq!(make_point.id, "src/main.zig::makePoint");

        let main = nodes
            .iter()
            .find(|n| n.name == "main" && n.kind == "function")
            .unwrap();
        assert_eq!(main.id, "src/main.zig::main");

        let edge_sum_helper = edges
            .iter()
            .find(|e| e.source_id == sum_method.id && e.target_id == helper_method.id)
            .unwrap();
        assert_eq!(edge_sum_helper.kind, "calls");

        let edge_main_make_point = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == make_point.id)
            .unwrap();
        assert_eq!(edge_main_make_point.kind, "calls");

        let edge_main_sum = edges
            .iter()
            .find(|e| e.source_id == main.id && e.target_id == sum_method.id)
            .unwrap();
        assert_eq!(edge_main_sum.kind, "calls");
    }

    #[test]
    fn test_parse_cpp_out_of_line_method() {
        // An out-of-line method defined inside its namespace should inherit the
        // namespace prefix so it matches the in-class declaration's qname.
        let cpp_code = r#"
namespace demo {
class Point {
public:
    int sum();
};

int Point::sum() {
    return 42;
}
}
"#;

        let (nodes, _calls) = parse_code("src/point.cpp", cpp_code, "cpp").unwrap();

        let sum_method = nodes
            .iter()
            .find(|n| n.name == "sum" && n.kind == "method")
            .unwrap();
        assert_eq!(sum_method.id, "src/point.cpp::demo::Point::sum");
    }

    #[test]
    fn test_parse_zig_keyword_in_string_is_not_a_type() {
        // The container-kind heuristic must not treat a string literal (or a
        // plain value) containing "struct"/"enum"/"union" as a type symbol.
        let zig_code = r#"
const message = "this mentions a struct and an enum";
const Real = struct {
    x: i32,
};
"#;

        let (nodes, _calls) = parse_code("src/main.zig", zig_code, "zig").unwrap();

        assert!(
            nodes.iter().all(|n| n.name != "message"),
            "string-valued const should not be indexed as a type"
        );
        assert!(
            nodes.iter().any(|n| n.name == "Real" && n.kind == "struct"),
            "genuine struct declaration should still be indexed"
        );
    }
}
