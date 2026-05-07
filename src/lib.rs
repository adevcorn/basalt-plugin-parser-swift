// swift-parser/src/lib.rs — tree-sitter Swift parser WASM plugin for Basalt

use tree_sitter::{Language, Parser, Query, QueryCursor};

const SRC_OFFSET: usize = 1 * 1024 * 1024;
const OUT_OFFSET: usize = 6 * 1024 * 1024;
const MEMORY_BYTES: usize = 12 * 1024 * 1024;

const SCOPE_KEYWORD: u8 = 1;
const SCOPE_STRING: u8 = 2;
const SCOPE_NUMBER: u8 = 3;
const SCOPE_COMMENT: u8 = 4;
const SCOPE_TYPE: u8 = 5;
const SCOPE_FUNCTION: u8 = 6;
const SCOPE_OPERATOR: u8 = 7;

static mut MEMORY: [u8; MEMORY_BYTES] = [0u8; MEMORY_BYTES];
static LANG_EXT: &[u8] = b"swift\0";

extern "C" { fn tree_sitter_swift() -> Language; }

#[no_mangle]
pub extern "C" fn basalt_lang() -> i32 {
    LANG_EXT.as_ptr() as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_parse(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_spans: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        (MEMORY.as_ptr() as usize + src_ptr as usize) as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        (MEMORY.as_ptr() as usize + out_ptr as usize) as *mut u8,
        (max_spans as usize) * 12,
    );

    let lang = tree_sitter_swift();
    let mut parser = Parser::new();
    if parser.set_language(lang).is_err() { return 0; }
    let Some(tree) = parser.parse(src, None) else { return 0; };

    let query_src = r#"
        "func" @keyword "let" @keyword "var" @keyword "class" @keyword
        "struct" @keyword "enum" @keyword "protocol" @keyword "extension" @keyword
        "actor" @keyword "import" @keyword "return" @keyword "if" @keyword
        "for" @keyword "while" @keyword "guard" @keyword "switch" @keyword
        (line_string_literal) @string (comment) @comment (multiline_comment) @comment
        (type_identifier) @type
        (function_declaration name: (_) @function)
    "#;
    let Ok(query) = Query::new(lang, query_src) else { return 0; };
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), src);
    let cap_names = query.capture_names().to_vec();

    let mut count = 0usize;
    for m in matches {
        for cap in m.captures {
            if count >= max_spans as usize { break; }
            let scope_id = match cap_names[cap.index as usize].as_str() {
                "keyword"  => SCOPE_KEYWORD,
                "string"   => SCOPE_STRING,
                "number"   => SCOPE_NUMBER,
                "comment"  => SCOPE_COMMENT,
                "type"     => SCOPE_TYPE,
                "function" => SCOPE_FUNCTION,
                "operator" => SCOPE_OPERATOR,
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

#[no_mangle]
pub unsafe extern "C" fn basalt_retrieval_chunks(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_chunks: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        (MEMORY.as_ptr() as usize + src_ptr as usize) as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        (MEMORY.as_ptr() as usize + out_ptr as usize) as *mut u8,
        (max_chunks as usize) * 104,
    );

    let lang = tree_sitter_swift();
    let mut parser = Parser::new();
    if parser.set_language(lang).is_err() { return 0; }
    let Some(tree) = parser.parse(src, None) else { return 0; };

    // tree-sitter-swift 0.3.x: struct/class/actor/enum/extension are all
    // class_declaration nodes, differentiated by the declaration_kind field.
    let query_src = r#"
        (function_declaration name: (_) @name.function) @chunk.function
        (class_declaration declaration_kind: "class"     name: (_) @name.type) @chunk.type
        (class_declaration declaration_kind: "struct"    name: (_) @name.type) @chunk.type
        (class_declaration declaration_kind: "enum"      name: (_) @name.type) @chunk.type
        (class_declaration declaration_kind: "actor"     name: (_) @name.type) @chunk.type
        (class_declaration declaration_kind: "extension" name: (_) @name.extension) @chunk.extension
        (protocol_declaration name: (_) @name.type) @chunk.type
    "#;
    let Ok(query) = Query::new(lang, query_src) else { return 0; };
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), src);
    let cap_names = query.capture_names().to_vec();

    let mut count = 0usize;
    for m in matches {
        if count >= max_chunks as usize { break; }
        let mut offset = None::<u32>;
        let mut length = None::<u32>;
        let mut kind = None::<&str>;
        let mut name = None::<&str>;
        for cap in m.captures {
            let cn = &cap_names[cap.index as usize];
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
        count += 1;
    }
    count as i32
}

#[no_mangle]
pub unsafe extern "C" fn basalt_call_sites(
    src_ptr: i32, src_len: i32, out_ptr: i32, max_sites: i32,
) -> i32 {
    let src = std::slice::from_raw_parts(
        (MEMORY.as_ptr() as usize + src_ptr as usize) as *const u8,
        src_len as usize,
    );
    let out = std::slice::from_raw_parts_mut(
        (MEMORY.as_ptr() as usize + out_ptr as usize) as *mut u8,
        (max_sites as usize) * 68,
    );

    let lang = tree_sitter_swift();
    let mut parser = Parser::new();
    if parser.set_language(lang).is_err() { return 0; }
    let Some(tree) = parser.parse(src, None) else { return 0; };

    let query_src = r#"
        (call_expression (simple_identifier) @callee)
        (call_expression (navigation_expression (navigation_suffix (simple_identifier) @callee)))
    "#;
    let Ok(query) = Query::new(lang, query_src) else { return 0; };
    let mut cursor = QueryCursor::new();
    let matches = cursor.matches(&query, tree.root_node(), src);

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
