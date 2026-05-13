// swift-parser/src/lib.rs — tree-sitter Swift parser WASM plugin for Basalt
//
// The host must call basalt_src_ptr() / basalt_out_ptr() after instantiation
// to obtain the linear-memory addresses to write/read.

use tree_sitter::{Language, Parser, Query, QueryCursor};

const SRC_OFFSET: usize = 0;
const OUT_OFFSET: usize = 6 * 1024 * 1024;
const MEMORY_BYTES: usize = 12 * 1024 * 1024;

const SCOPE_KEYWORD:   u8 = 1;
const SCOPE_STRING:    u8 = 2;
const SCOPE_NUMBER:    u8 = 3;
const SCOPE_COMMENT:   u8 = 4;
const SCOPE_TYPE:      u8 = 5;
const SCOPE_FUNCTION:  u8 = 6;
const SCOPE_OPERATOR:  u8 = 7;
const SCOPE_VARIABLE:  u8 = 10;
const SCOPE_NAMESPACE: u8 = 11;

static mut MEMORY: [u8; MEMORY_BYTES] = [0u8; MEMORY_BYTES];
static LANG_EXT: &[u8] = b"swift\0";

extern "C" { fn tree_sitter_swift() -> Language; }

// ---------------------------------------------------------------------------
// Static parser + query cache (WASM is single-threaded; safe to use static mut)
// ---------------------------------------------------------------------------

struct ParserState {
    parser: Parser,
    parse_query: Query,
    retrieval_query: Query,
    call_sites_query: Query,
    parse_cap_names: Vec<String>,
    retrieval_cap_names: Vec<String>,
}

static mut STATE: Option<ParserState> = None;

unsafe fn get_state() -> Option<&'static mut ParserState> {
    if STATE.is_none() {
        let lang = tree_sitter_swift();
        let mut parser = Parser::new();
        parser.set_language(lang).ok()?;

        let parse_query_src = r#"
            "func" @keyword "let" @keyword "var" @keyword "class" @keyword
            "struct" @keyword "enum" @keyword "protocol" @keyword "extension" @keyword
            "actor" @keyword "import" @keyword "return" @keyword "if" @keyword
            "for" @keyword "while" @keyword "guard" @keyword "switch" @keyword
            "typealias" @keyword "indirect" @keyword "nonisolated" @keyword
            "override" @keyword "convenience" @keyword "required" @keyword
            "some" @keyword "async" @keyword "await" @keyword
            "do" @keyword "break" @keyword "continue" @keyword
            "repeat" @keyword "case" @keyword "fallthrough" @keyword "nil" @keyword
            (else) @keyword
            (throws) @keyword (where_keyword) @keyword (as_operator) @keyword
            (boolean_literal) @keyword
            (visibility_modifier) @keyword
            (member_modifier) @keyword
            (function_modifier) @keyword
            (property_modifier) @keyword
            (parameter_modifier) @keyword
            (inheritance_modifier) @keyword
            (getter_specifier) @keyword
            (setter_specifier) @keyword
            (line_string_literal) @string
            (raw_string_literal) @string
            (comment) @comment (multiline_comment) @comment
            (integer_literal) @number (hex_literal) @number
            (oct_literal) @number (bin_literal) @number (real_literal) @number
            (type_identifier) @type
            (function_declaration name: (simple_identifier) @function)
            (call_expression (simple_identifier) @function)
            (call_expression
              (navigation_expression
                (navigation_suffix (simple_identifier) @function)))
            (directive) @macro
            (diagnostic) @macro
            "+" @operator "-" @operator "*" @operator "/" @operator
            "%" @operator "=" @operator "+=" @operator "-=" @operator
            "*=" @operator "/=" @operator "%=" @operator
            "==" @operator "!=" @operator "===" @operator "!==" @operator
            "<" @operator ">" @operator "<=" @operator ">=" @operator
            "&&" @operator "||" @operator "!" @operator
            "->" @operator "??" @operator "..<" @operator "..." @operator
            "++" @operator "--" @operator "&" @operator "~" @operator
            "try" @operator "try?" @operator "try!" @operator
            (custom_operator) @operator
            (pattern bound_identifier: (simple_identifier) @variable)
            (for_statement (pattern (simple_identifier) @variable))
            (parameter name: (simple_identifier) @variable)
            (parameter external_name: (simple_identifier) @variable)
            (navigation_expression (simple_identifier) @namespace
              (navigation_suffix))
        "#;
        let parse_query = Query::new(lang, parse_query_src).ok()?;
        let parse_cap_names: Vec<String> = parse_query.capture_names().iter().map(|s| s.to_string()).collect();

        let retrieval_query_src = r#"
            (function_declaration name: (_) @name.function) @chunk.function
            (class_declaration declaration_kind: "class"     name: (_) @name.type) @chunk.type
            (class_declaration declaration_kind: "struct"    name: (_) @name.type) @chunk.type
            (class_declaration declaration_kind: "enum"      name: (_) @name.type) @chunk.type
            (class_declaration declaration_kind: "actor"     name: (_) @name.type) @chunk.type
            (class_declaration declaration_kind: "extension" name: (_) @name.extension) @chunk.extension
            (protocol_declaration name: (_) @name.type) @chunk.type
        "#;
        let retrieval_query = Query::new(lang, retrieval_query_src).ok()?;
        let retrieval_cap_names: Vec<String> = retrieval_query.capture_names().iter().map(|s| s.to_string()).collect();

        let call_sites_query_src = r#"
            (call_expression (simple_identifier) @callee)
            (call_expression (navigation_expression (navigation_suffix (simple_identifier) @callee)))
        "#;
        let call_sites_query = Query::new(lang, call_sites_query_src).ok()?;

        STATE = Some(ParserState {
            parser,
            parse_query,
            retrieval_query,
            call_sites_query,
            parse_cap_names,
            retrieval_cap_names,
        });
    }
    STATE.as_mut()
}

// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn basalt_lang() -> i32 {
    LANG_EXT.as_ptr() as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_src_ptr() -> i32 {
    MEMORY[SRC_OFFSET..].as_ptr() as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_out_ptr() -> i32 {
    MEMORY[OUT_OFFSET..].as_ptr() as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_parse(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_spans: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        src_ptr as usize as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        out_ptr as usize as *mut u8,
        (max_spans as usize) * 12,
    );

    let state = match get_state() { Some(s) => s, None => return 0 };
    state.parser.reset();
    let Some(tree) = state.parser.parse(src, None) else { return 0 };

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&state.parse_query, tree.root_node(), src);

    let mut count = 0usize;
    for m in matches {
        for cap in m.captures {
            if count >= max_spans as usize { break; }
            let scope_id = match state.parse_cap_names[cap.index as usize].as_str() {
                "keyword"   => SCOPE_KEYWORD,
                "string"    => SCOPE_STRING,
                "number"    => SCOPE_NUMBER,
                "comment"   => SCOPE_COMMENT,
                "type"      => SCOPE_TYPE,
                "function"  => SCOPE_FUNCTION,
                "operator"  => SCOPE_OPERATOR,
                "macro"     => SCOPE_OPERATOR,
                "variable"  => SCOPE_VARIABLE,
                "namespace" => SCOPE_NAMESPACE,
                _ => 0,
            };
            let offset = cap.node.start_byte() as u32;
            let length = (cap.node.end_byte() - cap.node.start_byte()) as u32;
            let base = count * 12;
            out[base..base+4].copy_from_slice(&offset.to_le_bytes());
            out[base+4..base+8].copy_from_slice(&length.to_le_bytes());
            out[base+8] = scope_id;
            out[base+9] = 0; out[base+10] = 0; out[base+11] = 0;
            count += 1;
        }
    }
    count as i32
}

/// Map the tree-sitter capture kind string to the Basalt semantic node kind byte.
/// These values must match the `SEMANTIC_NODE_*` constants in `core/src/semantic_layer.rs`.
fn kind_byte(k: &str) -> u8 {
    match k {
        "module"    => 1,
        "type"      => 2,
        "function"  => 3,
        "extension" => 4,
        "property"  => 5,
        "enum_case" => 6,
        "interface" => 7,
        _           => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn basalt_retrieval_chunks(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_chunks: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        src_ptr as usize as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        out_ptr as usize as *mut u8,
        (max_chunks as usize) * 104,
    );

    let state = match get_state() { Some(s) => s, None => return 0 };
    state.parser.reset();
    let Some(tree) = state.parser.parse(src, None) else { return 0 };

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&state.retrieval_query, tree.root_node(), src);

    let mut count = 0usize;
    for m in matches {
        if count >= max_chunks as usize { break; }
        let mut offset = None::<u32>;
        let mut length = None::<u32>;
        let mut kind = None::<&str>;
        let mut name = None::<&str>;
        for cap in m.captures {
            let cn = &state.retrieval_cap_names[cap.index as usize];
            if let Some(k) = cn.strip_prefix("chunk.") {
                offset = Some(cap.node.start_byte() as u32);
                length = Some((cap.node.end_byte() - cap.node.start_byte()) as u32);
                kind = Some(k);
            } else if cn.starts_with("name.") {
                if let Ok(t) = cap.node.utf8_text(src) { name = Some(t.trim()); }
            }
        }
        let (Some(off), Some(len), Some(k)) = (offset, length, kind) else { continue };
        let label = if let Some(n) = name {
            let mut s = k.to_string(); s.push(' '); s.push_str(n); s
        } else { k.to_string() };
        let base = count * 104;
        out[base..base+4].copy_from_slice(&off.to_le_bytes());
        out[base+4..base+8].copy_from_slice(&len.to_le_bytes());
        let lbytes = label.as_bytes();
        let llen = lbytes.len().min(95);
        out[base+8..base+8+llen].copy_from_slice(&lbytes[..llen]);
        out[base+8+llen] = 0;
        // Byte 103 = kind u8 (0 = unset/infer from label; non-zero = explicit kind)
        out[base+103] = kind_byte(k);
        count += 1;
    }
    count as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_call_sites(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_sites: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        src_ptr as usize as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        out_ptr as usize as *mut u8,
        (max_sites as usize) * 68,
    );

    let state = match get_state() { Some(s) => s, None => return 0 };
    state.parser.reset();
    let Some(tree) = state.parser.parse(src, None) else { return 0 };

    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&state.call_sites_query, tree.root_node(), src);

    let mut count = 0usize;
    for m in matches {
        if count >= max_sites as usize { break; }
        for cap in m.captures {
            let Ok(name) = cap.node.utf8_text(src) else { continue };
            let name = name.trim();
            if name.is_empty() { continue; }
            let offset = cap.node.start_byte() as u32;
            let base = count * 68;
            out[base..base+4].copy_from_slice(&offset.to_le_bytes());
            let nb = name.as_bytes();
            let nlen = nb.len().min(63);
            out[base+4..base+4+nlen].copy_from_slice(&nb[..nlen]);
            out[base+4+nlen] = 0;
            count += 1;
        }
    }
    count as i32
}
