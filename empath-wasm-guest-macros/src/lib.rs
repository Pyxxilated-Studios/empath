use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::LitStr;

const fn wit_path_literal() -> &'static str {
    // Embed an absolute path to the canonical WIT file; this avoids inlining the
    // contents (which can trigger duplicate package errors in wit-bindgen) and
    // lets wit-bindgen read directly from disk.
    let wit_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../empath-wasm-guest/wit/empath.wit"
    );
    wit_path
}
/// Generate Empath guest bindings plus helper shims.
#[proc_macro]
pub fn bindings(input: TokenStream) -> TokenStream {
    if !input.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "bindings! does not take arguments",
        )
        .to_compile_error()
        .into();
    }

    let wit_path = LitStr::new(wit_path_literal(), Span::call_site());

    bindings_tokens(&wit_path).into()
}

fn bindings_tokens(wit_path: &LitStr) -> TokenStream2 {
    let base = base_tokens(wit_path);
    let context_impl = context_impl_tokens();
    let read_error_impl = read_error_impl_tokens();
    let helpers = helpers_tokens();
    let host_module = host_module_tokens();
    let message_module = message_module_tokens();

    quote! {
        #base
        #context_impl
        #read_error_impl
        #helpers
        #host_module
        #message_module
    }
}

fn base_tokens(wit_path: &LitStr) -> TokenStream2 {
    quote! {
        ::wit_bindgen::generate!({
            path: #wit_path,
            world: "validator",
        });

        pub use exports::empath::validator::events::{EventContext, Guest as EventsGuest};
        pub use exports::empath::validator::validation::{
            Guest as ValidationGuest,
            Rejection,
            ValidationContext,
            ValidationResult,
        };
        /// Backwards-compatible alias for validation contexts.
        pub type Context = ValidationContext;
    }
}

fn context_impl_tokens() -> TokenStream2 {
    quote! {
        impl ::empath_wasm_guest::context::ContextExt for ValidationContext {
            fn session_id(&self) -> &str {
                &self.id
            }

            fn sender(&self) -> Option<&str> {
                self.sender.as_deref()
            }

            fn recipients(&self) -> &[::alloc::string::String] {
                &self.recipients
            }

            fn tracking_id(&self) -> Option<&str> {
                self.tracking_id.as_deref()
            }

            fn transaction_pairs(&self) -> &[(
                ::alloc::string::String,
                ::alloc::string::String,
            )] {
                &self.transaction
            }

            fn transaction_value(&self, key: &str) -> Option<&str> {
                self.transaction
                    .iter()
                    .find_map(|(k, v)| if k == key { Some(v.as_str()) } else { None })
            }

            fn max_message_size(&self) -> usize {
                usize::try_from(self.max_message_size).unwrap_or(usize::MAX)
            }

            fn is_extended(&self) -> bool {
                self.extended
            }
        }

        impl ::empath_wasm_guest::context::ContextExt for EventContext {
            fn session_id(&self) -> &str {
                &self.id
            }

            fn sender(&self) -> Option<&str> {
                self.sender.as_deref()
            }

            fn recipients(&self) -> &[::alloc::string::String] {
                &self.recipients
            }

            fn tracking_id(&self) -> Option<&str> {
                self.tracking_id.as_deref()
            }

            fn transaction_pairs(&self) -> &[(
                ::alloc::string::String,
                ::alloc::string::String,
            )] {
                &self.transaction
            }

            fn transaction_value(&self, key: &str) -> Option<&str> {
                self.transaction
                    .iter()
                    .find_map(|(k, v)| if k == key { Some(v.as_str()) } else { None })
            }

            fn max_message_size(&self) -> usize {
                usize::try_from(self.max_message_size).unwrap_or(usize::MAX)
            }

            fn is_extended(&self) -> bool {
                self.extended
            }
        }
    }
}

fn read_error_impl_tokens() -> TokenStream2 {
    quote! {
        impl ::core::convert::From<crate::empath::validator::message::ReadError>
            for ::empath_wasm_guest::message_helpers::MessageReadError
        {
            fn from(err: crate::empath::validator::message::ReadError) -> Self {
                match err {
                    crate::empath::validator::message::ReadError::MissingData => {
                        Self::MissingData
                    }
                    crate::empath::validator::message::ReadError::ParseFailed => {
                        Self::ParseFailed
                    }
                    crate::empath::validator::message::ReadError::OutOfRange => {
                        Self::OutOfRange
                    }
                    crate::empath::validator::message::ReadError::HostUnavailable => {
                        Self::HostUnavailable
                    }
                }
            }
        }
    }
}

fn helpers_tokens() -> TokenStream2 {
    quote! {
        pub use ::empath_wasm_guest::prelude::*;

        #[must_use]
        pub fn accept() -> ValidationResult {
            ValidationResult::Accept
        }

        #[must_use]
        pub fn reject(code: u16, message: impl Into<::alloc::string::String>) -> ValidationResult {
            let normalized = ::empath_wasm_guest::result::normalize_rejection_code(code);
            ValidationResult::Reject(Rejection {
                code: u32::from(normalized),
                message: message.into(),
            })
        }

        #[must_use]
        pub fn reject_permanent(message: impl Into<::alloc::string::String>) -> ValidationResult {
            reject(550, message)
        }

        #[must_use]
        pub fn reject_temporary(message: impl Into<::alloc::string::String>) -> ValidationResult {
            reject(451, message)
        }
    }
}

fn host_module_tokens() -> TokenStream2 {
    quote! {
        pub mod host {
            pub use crate::empath::validator::host::OutboundBinding;

            pub fn log(message: impl AsRef<str>) {
                crate::empath::validator::host::log(message.as_ref());
            }

            pub fn session_set(key: &str, value: &str) {
                crate::empath::validator::host::session_set(key, value);
            }

            pub fn session_get(key: &str) -> Option<::alloc::string::String> {
                crate::empath::validator::host::session_get(key)
            }

            pub fn session_exists(key: &str) -> bool {
                crate::empath::validator::host::session_exists(key)
            }

            pub fn transaction_set(key: &str, value: &str) {
                crate::empath::validator::host::transaction_set(key, value);
            }

            pub fn transaction_get(key: &str) -> Option<::alloc::string::String> {
                crate::empath::validator::host::transaction_get(key)
            }

            pub fn transaction_exists(key: &str) -> bool {
                crate::empath::validator::host::transaction_exists(key)
            }

            pub fn outbound_bindings() -> ::alloc::vec::Vec<::alloc::string::String> {
                crate::empath::validator::host::outbound_bindings()
            }

            pub fn set_outbound_binding(
                binding: crate::empath::validator::host::OutboundBinding,
            ) -> bool {
                crate::empath::validator::host::set_outbound_binding(&binding)
            }
        }
    }
}

fn message_module_tokens() -> TokenStream2 {
    let message_read = message_read_tokens();
    let message_headers = message_header_tokens();
    let message_body = message_body_tokens();

    quote! {
        pub mod message {
            use ::empath_wasm_guest::message_helpers::{
                map_read_result, map_status, map_status_bool, parse_newline_style,
                MessageMutationError, MessageReadError, MutationResult, NewlineStyle, ReadResult,
            };

            const DEFAULT_READ_CHUNK: u32 = 64 * 1024;

            #message_read
            #message_headers
            #message_body
        }
    }
}

fn message_read_tokens() -> TokenStream2 {
    quote! {
        pub fn raw_len() -> ReadResult<u64> {
            map_read_result(crate::empath::validator::message::raw_len())
        }

        pub fn raw_read(offset: u64, len: u32) -> ReadResult<::alloc::vec::Vec<u8>> {
            map_read_result(crate::empath::validator::message::raw_read(offset, len))
        }

        pub fn raw_bytes() -> ReadResult<::alloc::vec::Vec<u8>> {
            let total = raw_len()?;
            let mut out = ::alloc::vec::Vec::with_capacity(usize::try_from(total).unwrap_or(0));
            let mut offset = 0u64;
            while offset < total {
                let remaining = total - offset;
                let to_read = remaining.min(u64::from(DEFAULT_READ_CHUNK)) as u32;
                let chunk = raw_read(offset, to_read)?;
                if chunk.is_empty() {
                    return Err(MessageReadError::Truncated);
                }
                offset = offset.saturating_add(chunk.len() as u64);
                out.extend_from_slice(&chunk);
            }
            Ok(out)
        }
    }
}

fn message_header_tokens() -> TokenStream2 {
    quote! {
        pub fn headers_get(name: &str) -> ::alloc::vec::Vec<::alloc::string::String> {
            crate::empath::validator::message::headers_get(name)
        }

        pub fn headers_contains(name: &str) -> bool {
            crate::empath::validator::message::headers_contains(name)
        }

        pub fn headers_set(name: &str, value: &str) -> MutationResult {
            map_status(crate::empath::validator::message::headers_set(name, value))
        }

        pub fn headers_append(name: &str, value: &str) -> MutationResult {
            map_status(crate::empath::validator::message::headers_append(name, value))
        }

        pub fn headers_set_if_absent(name: &str, value: &str) -> Result<bool, MessageMutationError> {
            map_status_bool(crate::empath::validator::message::headers_set_if_absent(
                name, value,
            ))
        }

        pub fn headers_remove(name: &str) -> u32 {
            crate::empath::validator::message::headers_remove(name)
        }

        pub fn headers_ensure_message_id() -> MutationResult {
            map_status(crate::empath::validator::message::headers_ensure_message_id())
        }

        pub fn headers_set_content_type(value: &str) -> MutationResult {
            map_status(crate::empath::validator::message::headers_set_content_type(value))
        }

        pub fn headers_set_transfer_encoding(value: &str) -> MutationResult {
            map_status(crate::empath::validator::message::headers_set_transfer_encoding(value))
        }
    }
}

fn message_body_tokens() -> TokenStream2 {
    quote! {
        pub fn body_len() -> ReadResult<u64> {
            map_read_result(crate::empath::validator::message::body_len())
        }

        pub fn body_read(offset: u64, len: u32) -> ReadResult<::alloc::vec::Vec<u8>> {
            map_read_result(crate::empath::validator::message::body_read(offset, len))
        }

        pub fn body_bytes() -> ReadResult<::alloc::vec::Vec<u8>> {
            let total = body_len()?;
            let mut out = ::alloc::vec::Vec::with_capacity(usize::try_from(total).unwrap_or(0));
            let mut offset = 0u64;
            while offset < total {
                let remaining = total - offset;
                let to_read = remaining.min(u64::from(DEFAULT_READ_CHUNK)) as u32;
                let chunk = body_read(offset, to_read)?;
                if chunk.is_empty() {
                    return Err(MessageReadError::Truncated);
                }
                offset = offset.saturating_add(chunk.len() as u64);
                out.extend_from_slice(&chunk);
            }
            Ok(out)
        }

        pub fn body_newline_style() -> NewlineStyle {
            parse_newline_style(&crate::empath::validator::message::body_newline_style())
        }

        pub fn body_set_bytes(data: &[u8]) -> MutationResult {
            map_status(crate::empath::validator::message::body_set_bytes(data))
        }

        pub fn body_append_bytes(data: &[u8]) -> MutationResult {
            map_status(crate::empath::validator::message::body_append_bytes(data))
        }

        pub fn body_set_text(text: &str, charset: &str) -> MutationResult {
            map_status(crate::empath::validator::message::body_set_text(text, charset))
        }

        pub fn body_append_text(text: &str, charset: &str) -> MutationResult {
            map_status(crate::empath::validator::message::body_append_text(text, charset))
        }

        pub fn body_set_html(html: &str, charset: &str) -> MutationResult {
            map_status(crate::empath::validator::message::body_set_html(html, charset))
        }

        pub fn body_append_html(html: &str, charset: &str) -> MutationResult {
            map_status(crate::empath::validator::message::body_append_html(html, charset))
        }

        pub fn body_set_alternative(text: &str, html: &str, charset: &str) -> MutationResult {
            map_status(crate::empath::validator::message::body_set_alternative(
                text, html, charset,
            ))
        }
    }
}
