//! Embedded Rust syntax-contract guard for live hot patching.
//!
//! Only non-const, inert-attributed free function bodies (excluding `main`) and inherent `impl`
//! method bodies may change without a restart. Any change to types, statics, constants, const fn
//! bodies, function signatures, trait impls, imports, unknown attributes, or non-safe macros
//! triggers a rebuild and restart.

use quote::ToTokens;
use syn::parse::Parser;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardOutcome {
    Compatible,
    RestartNeeded,
    SyntaxError(String),
}

#[derive(Default)]
struct Contracts(Vec<String>);

impl<'ast> Visit<'ast> for Contracts {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        // Local types, statics, functions and imports are also contracts. Do not erase them.
        self.0.push(item.to_token_stream().to_string());
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        // Unknown macros could expand to persistent types/statics. Keep their entire invocation.
        const SAFE: &[&str] = &[
            "format",
            "format_args",
            "vec",
            "matches",
            "println",
            "eprintln",
            "assert",
            "assert_eq",
            "debug_assert",
            "panic",
            "unreachable",
            "todo",
        ];
        if m.path
            .get_ident()
            .is_some_and(|ident| SAFE.iter().any(|s| ident == s))
        {
            // syn does not visit macro tokens. Parse their expressions explicitly so a
            // local type/static or nested unknown macro cannot hide inside vec!/format!.
            if m.path.is_ident("vec") {
                let tokens = &m.tokens;
                if let Ok(expr) = syn::parse2::<syn::Expr>(quote::quote!([#tokens])) {
                    self.visit_expr(&expr);
                    return;
                }
            } else if let Ok(args) =
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                    .parse2(m.tokens.clone())
            {
                for arg in args {
                    self.visit_expr(&arg);
                }
                return;
            }
        }
        self.0.push(m.to_token_stream().to_string());
    }
}

fn inert(attrs: &[syn::Attribute]) -> bool {
    // Unknown attributes can expand a body into types or statics, even on an ordinary function.
    const INERT: &[&str] = &["doc", "inline", "cold", "allow"];
    attrs.iter().all(|a| {
        a.path()
            .get_ident()
            .is_some_and(|ident| INERT.iter().any(|s| ident == s))
    })
}

#[derive(Default)]
struct Scrub(Contracts);

impl Scrub {
    fn body(&mut self, sig: &syn::Signature, attrs: &[syn::Attribute], block: &mut syn::Block) {
        // Const bodies can determine persistent layouts.
        if sig.constness.is_some() || !inert(attrs) {
            return;
        }
        self.0.visit_block(block);
        *block = syn::parse_quote!({});
    }
}

impl VisitMut for Scrub {
    fn visit_item_fn_mut(&mut self, f: &mut syn::ItemFn) {
        if f.sig.ident == "main" {
            return;
        }
        self.body(&f.sig, &f.attrs, &mut f.block);
    }

    fn visit_item_impl_mut(&mut self, i: &mut syn::ItemImpl) {
        // Inherent method bodies are layout neutral like free fn bodies. Trait impls stay
        // exact (dispatch tables), as do impl attributes, headers, generics and other items.
        if i.trait_.is_some() || !inert(&i.attrs) {
            return;
        }
        for item in &mut i.items {
            if let syn::ImplItem::Fn(f) = item {
                self.body(&f.sig, &f.attrs, &mut f.block);
            }
        }
    }
}

/// Computes the persistent layout/ABI contract fingerprint of a Rust source file.
pub fn contract(src: &str) -> Result<String, syn::Error> {
    let mut ast = syn::parse_file(src)?;
    let mut scrub = Scrub::default();
    scrub.visit_file_mut(&mut ast);
    Ok(format!("{}\n{:?}", ast.to_token_stream(), scrub.0.0))
}

/// Compares a source file before and after an edit to decide whether live patching is safe.
pub fn check_edit(before: &str, after: &str) -> GuardOutcome {
    match (contract(before), contract(after)) {
        (Ok(a), Ok(b)) if a == b => GuardOutcome::Compatible,
        (Ok(_), Ok(_)) => GuardOutcome::RestartNeeded,
        (_, Err(e)) | (Err(e), _) => GuardOutcome::SyntaxError(e.to_string()),
    }
}
