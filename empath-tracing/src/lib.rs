#![feature(fn_traits, unboxed_closures)]
#![warn(clippy::pedantic)]

extern crate proc_macro;

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::{parse::Parse, parse_macro_input, parse_quote, ItemFn, Stmt};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Precision {
    Nanos,
    Micros,
    Millis,
    Seconds,
    Unspecified,
}

impl FnOnce<()> for Precision {
    type Output = syn::Expr;

    extern "rust-call" fn call_once(self, _args: ()) -> Self::Output {
        match self {
            Self::Nanos => {
                parse_quote!(|d: std::time::Duration| format!("{} ns elapsed", d.as_nanos()))
            }
            Self::Micros => {
                parse_quote!(|d: std::time::Duration| format!("{} us elapsed", d.as_micros()))
            }
            Self::Millis => {
                parse_quote!(|d: std::time::Duration| format!("{} ms elapsed", d.as_millis()))
            }
            Self::Seconds => {
                parse_quote!(|d: std::time::Duration| format!("{} s elapsed", d.as_secs()))
            }
            Self::Unspecified => parse_quote!(|_: std::time::Duration| String::default()),
        }
    }
}

impl From<&str> for Precision {
    fn from(value: &str) -> Self {
        match value {
            "ns" | "nano" | "nanos" | "nanoseconds" => Self::Nanos,
            "us" | "micro" | "micros" | "microseconds" => Self::Micros,
            "ms" | "milli" | "millis" | "milliseconds" => Self::Millis,
            "s" | "sec" | "secs" | "seconds" => Self::Seconds,
            _ => Self::Unspecified,
        }
    }
}

impl Default for Precision {
    fn default() -> Self {
        Self::Nanos
    }
}

impl Parse for Precision {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.peek(syn::token::Paren) {
            let content;
            let _ = syn::parenthesized!(content in input);
            let _ = content.parse::<keywords::precision>()?;
            let _ = content.parse::<syn::Token![=]>()?;
            let precision = content.parse::<syn::LitStr>()?.value();

            Ok(Self::from(precision.as_str()))
        } else {
            Ok(Self::default())
        }
    }
}

#[derive(Default)]
struct Attributes {
    timing: Option<Precision>,
    instrument: Option<TokenStream>,
    warnings: Vec<syn::Error>,
}

mod keywords {
    syn::custom_keyword!(timing);
    syn::custom_keyword!(precision);
    syn::custom_keyword!(instrument);
}

impl Attributes {
    pub(crate) fn warnings(&self) -> impl quote::ToTokens {
        let warnings = self.warnings.iter().map(|err| {
            let msg = format!("found unrecognized input, {err}");
            let msg = syn::LitStr::new(&msg, err.span());

            quote_spanned! {err.span()=>
                #[warn(deprecated)]
                {
                    #[deprecated(since = "not actually deprecated", note = #msg)]
                    const TRACING_INSTRUMENT_WARNING: () = ();
                    let _ = TRACING_INSTRUMENT_WARNING;
                }
            }
        });
        quote! {
            { #(#warnings)* }
        }
    }
}

impl Parse for Attributes {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut attributes = Self::default();

        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(keywords::timing) {
                if attributes.timing.is_some() {
                    return Err(input.error("expected only a single `precision` argument"));
                }

                let _ = input.parse::<keywords::timing>()?;
                attributes.timing = Some(input.parse()?);
            } else if lookahead.peek(keywords::instrument) {
                if attributes.instrument.is_some() {
                    return Err(input.error("expected only a single `instrument` argument"));
                }

                let _ = input.parse::<keywords::instrument>()?;
                if input.peek(syn::token::Paren) {
                    let content;
                    let _ = syn::parenthesized!(content in input);
                    attributes.instrument = Some(content.parse()?);
                }
            } else if lookahead.peek(syn::Token![,]) {
                let _ = input.parse::<syn::Token![,]>()?;
            } else {
                // Emit a warning for a token we didn't expect
                attributes.warnings.push(lookahead.error());
                // Throw it away and keep parsing
                let _ = input.parse::<proc_macro2::TokenTree>();
            }
        }

        Ok(attributes)
    }
}

/// Adds `log::trace!` events at the start and end of an attributed function.
///
/// # Panics
///
/// When applied to anything other than a function.
#[proc_macro_attribute]
pub fn traced(
    args: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let args = parse_macro_input!(args as Attributes);
    let warnings = args.warnings();

    let mut item_fn = parse_macro_input!(item as ItemFn);

    let clippy_attr: syn::Attribute = parse_quote! {
        #[allow(clippy::items_after_statements)]
    };
    item_fn.attrs.push(clippy_attr);

    if args.instrument.is_some() {
        let fields = args.instrument.unwrap();
        let fields = fields.to_token_stream();
        let instrument_attr: syn::Attribute = parse_quote! {
            #[tracing::instrument(#fields)]
        };
        item_fn.attrs.push(instrument_attr);
    }

    let id = item_fn.sig.ident.to_string();
    let timing: Stmt = if args.timing.is_none() {
        parse_quote! { tracing::trace!("OnExit: {}", #id); }
    } else {
        let timing = args.timing.unwrap()();
        parse_quote! { tracing::trace!("OnExit: {} ({})", #id, (#timing)(self.timer.elapsed())); }
    };

    let decl: Vec<Stmt> = parse_quote! {
        struct __Instrument {
            timer: std::time::Instant,
        }

        impl __Instrument {
            fn new() -> Self {
                #warnings

                tracing::trace!("OnEnter: {}", #id);
                __Instrument {
                    timer: std::time::Instant::now(),
                }
            }
        }

        impl std::ops::Drop for __Instrument {
            fn drop(&mut self) {
                #timing
            }
        }
    };

    let init: Stmt = parse_quote! { let __instrument = __Instrument::new(); };
    item_fn.block.stmts.insert(0, init);
    decl.into_iter()
        .rev()
        .for_each(|s| item_fn.block.stmts.insert(0, s));

    let out = quote! { #item_fn };
    proc_macro::TokenStream::from(out)
}
