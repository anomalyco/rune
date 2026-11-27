//! A syntax guard, not a size_of guess: same-size field/type/signature edits are incompatible too.
use quote::ToTokens;
use syn::{parse::Parser, visit::Visit, visit_mut::VisitMut};

#[derive(Default)]
struct Contracts(Vec<String>);
impl<'ast> Visit<'ast> for Contracts {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        // Local types, statics, functions and imports are also contracts. Do not erase them.
        self.0.push(item.to_token_stream().to_string());
    }
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        // Unknown macros could expand to persistent types/statics. Keep their entire invocation.
        let safe = ["format", "format_args", "vec", "matches", "println", "eprintln", "assert", "assert_eq", "debug_assert", "panic", "unreachable", "todo"];
        if m.path.get_ident().is_some_and(|i| safe.iter().any(|s| i == s)) {
            // syn does not visit macro tokens. Parse their expressions explicitly so a
            // local type/static or nested unknown macro cannot hide inside vec!/format!.
            if m.path.is_ident("vec") {
                let tokens = &m.tokens;
                if let Ok(expr) = syn::parse2::<syn::Expr>(quote::quote!([#tokens])) {
                    self.visit_expr(&expr);
                    return;
                }
            } else if let Ok(args) = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated.parse2(m.tokens.clone()) {
                for arg in args { self.visit_expr(&arg); }
                return;
            }
        }
        self.0.push(m.to_token_stream().to_string());
    }
}

fn inert(attrs: &[syn::Attribute]) -> bool {
    // Unknown attributes can expand a body into types or statics, even on an ordinary function.
    let inert = ["doc", "inline", "cold", "allow"];
    attrs.iter().all(|a| a.path().get_ident().is_some_and(|i| inert.iter().any(|s| i == s)))
}

#[derive(Default)]
struct Scrub(Contracts);
impl Scrub {
    fn body(&mut self, sig: &syn::Signature, attrs: &[syn::Attribute], block: &mut syn::Block) {
        // Const bodies can determine persistent layouts.
        if sig.constness.is_some() || !inert(attrs) { return; }
        self.0.visit_block(block);
        *block = syn::parse_quote!({});
    }
}
impl VisitMut for Scrub {
    fn visit_item_fn_mut(&mut self, f: &mut syn::ItemFn) {
        if f.sig.ident == "main" { return; }
        self.body(&f.sig, &f.attrs, &mut f.block);
    }
    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        // Inherent method bodies are layout neutral like free fn bodies. Trait impls stay
        // exact (dispatch tables), as do impl attributes, headers, generics and other items.
        if i.trait_.is_some() || !inert(&i.attrs) { return; }
        for item in &mut i.items {
            if let syn::ImplItem::Fn(f) = item { self.body(&f.sig, &f.attrs, &mut f.block); }
        }
    }
    // Initializers, constants, attributes and module declarations stay byte-token exact.
}

fn contract(src: &str) -> Result<String, syn::Error> {
    let mut ast = syn::parse_file(src)?;
    let mut scrub = Scrub::default();
    scrub.visit_file_mut(&mut ast);
    Ok(format!("{}\n{:?}", ast.to_token_stream(), scrub.0.0))
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 2, "rune-dev-guard BEFORE.rs AFTER.rs");
    let read = |p: &str| contract(&std::fs::read_to_string(p).expect("read source"));
    match (read(&args[0]), read(&args[1])) {
        (Ok(a), Ok(b)) if a == b => {}
        (Ok(_), Ok(_)) => { eprintln!("restart needed: declaration, state schema, signature, startup code or persistent macro changed"); std::process::exit(3); }
        (_, Err(e)) | (Err(e), _) => { eprintln!("edit not ready: {e}"); std::process::exit(2); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compatible_bodies_but_not_state_or_boundary_changes() {
        let base = "struct S { a: u32, b: u32 } fn view(s: &S) -> u32 { s.a + 1 } fn main() { let s = S { a: 1, b: 2 }; }";
        assert_eq!(contract(base).unwrap(), contract(&base.replace("s.a + 1", "s.b + 20")).unwrap());
        for edit in [base.replace("a: u32, b: u32", "b: u32, a: u32"), base.replace("s: &S", "s: &mut S"), base.replace("a: 1", "a: 9"), base.replace("s.a + 1", "static X: u32 = 3; s.a + X"), base.replace("s.a + 1", "mystery!(s)")] {
            assert_ne!(contract(base).unwrap(), contract(&edit).unwrap());
        }
    }
    #[test]
    fn nested_types_and_impls_cannot_change() {
        let base = "fn view() { struct S { x: u32 } } impl T for S { fn init() -> u32 { 1 } }";
        assert_ne!(contract(base).unwrap(), contract(&base.replace("x: u32", "x: f32")).unwrap());
        assert_ne!(contract(base).unwrap(), contract(&base.replace("{ 1 }", "{ 2 }")).unwrap());
        assert!(contract("fn view( {").is_err());
    }

    #[test]
    fn inherent_method_bodies_patch_but_headers_and_signatures_do_not() {
        let base = "struct S; impl S { const N: u32 = 1; fn title(&self) -> &'static str { \"Home\" } }";
        assert_eq!(contract(base).unwrap(), contract(&base.replace("\"Home\"", "\"Start\"")).unwrap());
        for edit in [
            base.replace("&self", "&mut self"),
            base.replace("impl S", "impl<X> S"),
            base.replace("N: u32 = 1", "N: u32 = 2"),
            base.replace("\"Home\"", "{ static X: &str = \"Start\"; X }"),
        ] {
            assert_ne!(contract(base).unwrap(), contract(&edit).unwrap());
        }
        for kept in [
            "impl S { const fn n() -> usize { 1 } }",
            "impl S { #[persistent] fn n() -> usize { 1 } }",
            "#[persistent] impl S { fn n() -> usize { 1 } }",
            "impl T for S { fn n() -> usize { 1 } }",
        ] {
            assert_ne!(contract(kept).unwrap(), contract(&kept.replace("{ 1 }", "{ 2 }")).unwrap());
        }
    }

    #[test]
    fn inherent_method_named_main_patches_while_free_main_does_not() {
        let base = "struct Layout; impl Layout { fn main(&self) -> &'static str { \"Home\" } } struct App; impl App { fn main() -> u32 { 1 } } fn main() { let x = 1; }";
        assert_eq!(contract(base).unwrap(), contract(&base.replace("\"Home\"", "\"Start\"")).unwrap());
        assert_eq!(contract(base).unwrap(), contract(&base.replace("{ 1 }", "{ 2 }")).unwrap());
        assert_ne!(contract(base).unwrap(), contract(&base.replace("let x = 1;", "let x = 2;")).unwrap());
        assert_ne!(contract(base).unwrap(), contract(&base.replace("\"Home\"", "{ static X: &str = \"Start\"; X }")).unwrap());
        for kept in [
            "impl Layout { const fn main() -> usize { 1 } }",
            "impl Layout { #[persistent] fn main(&self) -> usize { 1 } }",
            "#[persistent] impl Layout { fn main(&self) -> usize { 1 } }",
            "impl T for Layout { fn main(&self) -> usize { 1 } }",
        ] {
            assert_ne!(contract(kept).unwrap(), contract(&kept.replace("{ 1 }", "{ 2 }")).unwrap());
        }
    }

    #[test]
    fn const_function_bodies_are_layout_contracts() {
        for layout in ["struct S { buf: [u8; cap()] }", "const N: usize = cap(); struct S { buf: [u8; N] }"] {
            let base = format!("const fn cap() -> usize {{ 8 }} {layout} fn view() {{}}");
            assert_ne!(contract(&base).unwrap(), contract(&base.replace("{ 8 }", "{ 16 }")).unwrap());
        }
    }

    #[test]
    fn unknown_attributes_keep_bodies_but_inert_attributes_allow_edits() {
        for attribute in ["#[persistent]", "#[some::macro_attr]", "#[cfg_attr(feature = \"x\", persistent)]"] {
            let base = format!("{attribute} fn view() {{ let n = 1; }}");
            assert_ne!(contract(&base).unwrap(), contract(&base.replace("n = 1", "n = 2")).unwrap());
        }
        let base = "#[doc = \"view\"] #[inline(never)] #[cold] #[allow(unused)] fn view() { let n = 1; }";
        assert_eq!(contract(base).unwrap(), contract(&base.replace("n = 1", "n = 2")).unwrap());
        assert_ne!(contract(base).unwrap(), contract(&base.replace("inline(never)", "inline(always)")).unwrap());
    }

    #[test]
    fn macro_arguments_cannot_hide_persistent_contracts() {
        for body in ["vec![{ static X: u32 = 1; X }]", "format!(\"{}\", { struct S { x: u32 } 1 })", "vec![unknown!(1)]"] {
            let base = format!("fn view() {{ {body}; }}");
            assert_ne!(contract(&base).unwrap(), contract(&base.replace("u32", "f32").replace("unknown!(1)", "unknown!(2)")).unwrap());
        }
        assert_eq!(contract("fn view() { vec![1, 2]; }").unwrap(), contract("fn view() { vec![3; 4]; }").unwrap());
    }
}
