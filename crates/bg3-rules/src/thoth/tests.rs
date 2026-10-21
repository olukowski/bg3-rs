//! Unit tests for the Thoth parser and interpreter (self-authored snippets).

use super::interp::{Machine, NoHost};
use super::value::Value;
use super::{parse_chunk, parse_expr};

fn run(src: &str) -> Vec<Value> {
    let chunk = parse_chunk(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    Machine::new().exec(&mut NoHost, chunk, Vec::new()).unwrap()
}

fn run1(src: &str) -> String {
    run(src).first().map(|v| v.to_string()).unwrap_or_default()
}

#[test]
fn parses_lua_53_syntax() {
    let src = r#"
        local a, b = 1, 2.5
        local t = { x = 1, [2] = "two", 3; nested = { 1, 2 } }
        function t.f(self, ...) return select('#', ...) end
        function t:g() return self end
        local s = [[long
        string]] .. 'q' .. "\x41\65\u{42}"
        --[==[ long
        comment ]==]
        for i = 1, 3 do if i == 2 then break end end
        for k, v in pairs(t) do end
        while false do end
        repeat local z = 1 until z == 1
        goto done
        ::done::
        return a // 1, b ^ 2, ~5, 1 << 2, 7 & 3 | 8 ~ 1, #t, not nil, -a
    "#;
    parse_chunk(src).unwrap();
}

#[test]
fn parses_try_catch() {
    let src = "try\n x = error('boom')\ncatch e then\n y = e\nend\nreturn y";
    assert_eq!(run1(src), "boom");
}

#[test]
fn rejects_bad_syntax() {
    assert!(parse_chunk("return (1").is_err());
    assert!(parse_chunk("x = = 1").is_err());
    assert!(parse_chunk("try x = 1 end").is_err());
}

#[test]
fn operator_precedence_matches_lua_53() {
    // Comparisons bind looser than bitwise operators.
    assert_eq!(run1("return 1 == 1 & 1"), "true");
    assert_eq!(run1("return 2 + 3 * 4 ^ 2"), "50.0");
    assert_eq!(run1("return 2 ^ 3 ^ 2"), "512.0");
    assert_eq!(run1("return -2 ^ 2"), "-4.0");
    assert_eq!(run1("return 1 .. 2 .. 3"), "123");
    assert_eq!(run1("return 1 | 2 & 3"), "3");
}

#[test]
fn integer_and_float_semantics() {
    assert_eq!(run1("return 7 // 2"), "3");
    assert_eq!(run1("return -7 // 2"), "-4");
    assert_eq!(run1("return 7 / 2"), "3.5");
    assert_eq!(run1("return -7 % 3"), "2");
    assert_eq!(run1("return math.floor(-3.5)"), "-4");
    assert_eq!(run1("return math.floor((15 - 10) / 2)"), "2");
    assert_eq!(run1("return math.max(1, 5, 3)"), "5");
    assert_eq!(run1("return 10 == 10.0"), "true");
}

#[test]
fn closures_capture_lexically() {
    let src = "local x = 1\nlocal f = function() return x end\nlocal x = 2\nreturn f(), x";
    let v = run(src);
    assert_eq!(v[0].to_string(), "1");
    assert_eq!(v[1].to_string(), "2");
    let counter = "local function mk() local n = 0 return function() n = n + 1 return n end end\n\
                   local c = mk() c() c() return c()";
    assert_eq!(run1(counter), "3");
}

#[test]
fn varargs_and_multiple_returns() {
    assert_eq!(
        run1("local function f(...) return select('#', ...) end return f(1, nil, 3)"),
        "3"
    );
    assert_eq!(
        run1("local function f() return 1, 2 end local t = { f() } return #t"),
        "2"
    );
    assert_eq!(
        run1("local function f() return 1, 2 end local t = { (f()) } return #t"),
        "1"
    );
    assert_eq!(run1("return (...)"), "nil");
}

#[test]
fn tables_and_metatables() {
    let src = "local base = { greet = function() return 'hi' end }\n\
               local t = setmetatable({}, { __index = base })\n\
               return t.greet()";
    assert_eq!(run1(src), "hi");
    let src = "local t = {} table.insert(t, 'a') table.insert(t, 'b') return table.concat(t, ',')";
    assert_eq!(run1(src), "a,b");
    let src = "local n = 0 for _, v in ipairs({ 5, 6, 7 }) do n = n + v end return n";
    assert_eq!(run1(src), "18");
}

#[test]
fn string_library_subset() {
    assert_eq!(run1("return (string.gsub('a and b', ' and ', '&'))"), "a&b");
    assert_eq!(
        run1("return (string.gsub('x)and y', '%)and ', ')&'))"),
        "x)&y"
    );
    assert_eq!(run1("return (('abc'):gsub('b+', 'X'))"), "aXc");
    assert_eq!(run1("return string.format('%s=%d', 'n', 3)"), "n=3");
    assert_eq!(run1("return ('hello'):sub(2, 3)"), "el");
}

#[test]
fn errors_and_pcall() {
    assert_eq!(run1("return pcall(error, 'x')"), "false");
    let chunk = parse_chunk("return nosuch()").unwrap();
    let err = Machine::new().exec(&mut NoHost, chunk, vec![]).unwrap_err();
    assert!(err.to_string().contains("nosuch"));
}

#[test]
fn parse_expr_single_expression() {
    assert!(parse_expr("a.b(c) & ~d").is_ok());
    assert!(parse_expr("a b").is_err());
}
