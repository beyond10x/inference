//! The attribute macros of the `inference` SDK, re-exported as `inference::tool` and
//! `inference::main` (`docs/design/sdk.md` §3.2, §5).
//!
//! Each macro emits only `::inference::…` paths, so an application with the one `inference`
//! dependency compiles what they generate: the tool contract at `::inference::tools::…`, and the
//! crates the expansions reach at `::inference::__private::…`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, FnArg, ItemFn, Lit, LitStr, Meta, Pat, PathArguments, ReturnType,
    Type, parse_macro_input,
};

/// `#[inference::tool]`: an `async fn` becomes an `inference::tools::Tool` and its executor.
///
/// The function is replaced by a function of the same name and visibility that takes no
/// parameters and returns the pair
/// `(::inference::tools::Tool, Box<dyn ::inference::tools::ToolExecutor>)`, so `weather()` is the
/// tool `weather` and its executor, and a local binding or an argument named like a tool shadows
/// it as it shadows any function. The arguments and the executor are types without a name outside
/// that function. The tool's name is the function's, without the `r#` of a raw identifier, its
/// description the doc comment, its schema the JSON schema of the arguments (an object, with
/// `properties` even when there are none, and every non-recursive subschema inlined; a recursive
/// argument keeps `$defs` and `$ref`, which the tool authority refuses at preparation), each of
/// which is `Deserialize`. The function and the pair it returns are both
/// `::inference::tools::IntoTool`, so a caller taking that accepts `weather` and `weather()`. The executor reads the call's arguments into them, runs the
/// body and answers its value, which is `Serialize`; a body returning `Result` answers `Ok`'s
/// value and fails with `ExecutionError` on `Err`, whose `Display` is the failure's detail, as
/// are arguments that do not read.
///
/// `effect = "read_only"` (the default), `"idempotent_write"` or `"non_idempotent_write"` sets the
/// tool's side-effect class; `confirm` makes it require human confirmation.
#[proc_macro_attribute]
pub fn tool(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let mut options = ToolOptions::default();
    let parser = syn::meta::parser(|meta| options.parse(&meta));
    parse_macro_input!(attribute with parser);
    let function = parse_macro_input!(item as ItemFn);
    expand_tool(&options, function)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `#[inference::main]`: an `async fn main(host: impl AudioHost, stack: Stack)` becomes the
/// program and its `run` seam.
///
/// It emits `pub async fn run(host, stack) -> ::inference::Result<()>` holding the body, with
/// `Ok(())` after it so `?` works inside, and a `fn main` that runs it on a multi-thread runtime
/// over the default audio host and `::inference::Stack::from_env()`, printing an error through
/// `::inference::__private::exit`. Any other signature is a compile error naming this one.
#[proc_macro_attribute]
pub fn main(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let attribute = TokenStream2::from(attribute);
    let function = parse_macro_input!(item as ItemFn);
    expand_main(&attribute, function)
        .unwrap_or_else(|error| {
            let error = error.into_compile_error();
            quote! {
                #error
                fn main() {}
            }
        })
        .into()
}

/// The side-effect classes `effect` names, each with its variant.
const EFFECTS: [(&str, &str); 3] = [
    ("read_only", "ReadOnly"),
    ("idempotent_write", "IdempotentWrite"),
    ("non_idempotent_write", "NonIdempotentWrite"),
];

/// The options of `#[inference::tool(...)]`.
#[derive(Default)]
struct ToolOptions {
    /// The variant of the side-effect class; read-only when none was given.
    effect: Option<&'static str>,
    confirm: bool,
}

impl ToolOptions {
    fn parse(&mut self, meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
        if meta.path.is_ident("effect") {
            let value: LitStr = meta.value()?.parse()?;
            let named = value.value();
            let variant = EFFECTS
                .iter()
                .find(|(name, _)| *name == named)
                .map(|(_, variant)| *variant)
                .ok_or_else(|| {
                    syn::Error::new(
                        value.span(),
                        format!(
                            "unknown effect {named:?}: expected \"read_only\", \"idempotent_write\" \
                             or \"non_idempotent_write\""
                        ),
                    )
                })?;
            if self.effect.replace(variant).is_some() {
                return Err(meta.error("`effect` is given twice"));
            }
            Ok(())
        } else if meta.path.is_ident("confirm") {
            if self.confirm {
                return Err(meta.error("`confirm` is given twice"));
            }
            self.confirm = true;
            Ok(())
        } else {
            Err(meta.error("unknown option: expected `effect = \"...\"` or `confirm`"))
        }
    }
}

/// One argument of a tool function: its name and type.
struct Argument {
    name: syn::Ident,
    ty: Type,
}

fn arguments(function: &ItemFn) -> syn::Result<Vec<Argument>> {
    function
        .sig
        .inputs
        .iter()
        .map(|input| match input {
            FnArg::Receiver(receiver) => Err(syn::Error::new(
                receiver.span(),
                "a tool is a free function and takes no `self`",
            )),
            FnArg::Typed(typed) => match typed.pat.as_ref() {
                Pat::Ident(ident) if ident.by_ref.is_none() && ident.subpat.is_none() => {
                    Ok(Argument {
                        name: ident.ident.clone(),
                        ty: typed.ty.as_ref().clone(),
                    })
                }
                other => Err(syn::Error::new(
                    other.span(),
                    "a tool argument is a plain name, which names it in the schema",
                )),
            },
        })
        .collect()
}

/// The doc comment of `attributes`, one line per line, the leading space of each removed and the
/// whole trimmed.
fn description(attributes: &[Attribute]) -> String {
    let lines: Vec<String> = attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| match &attribute.meta {
            Meta::NameValue(pair) => match &pair.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text),
                    ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|text| {
            text.split('\n')
                .map(|line| line.strip_prefix(' ').unwrap_or(line).trim_end().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    lines.join("\n").trim().to_owned()
}

/// Whether the function returns a `Result`, by the last segment of its return type's path.
fn returns_result(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    let Type::Path(path) = ty.as_ref() else {
        return false;
    };
    path.qself.is_none()
        && path.path.segments.last().is_some_and(|segment| {
            segment.ident == "Result"
                && matches!(segment.arguments, PathArguments::AngleBracketed(_))
        })
}

fn expand_tool(options: &ToolOptions, function: ItemFn) -> syn::Result<TokenStream2> {
    let signature = &function.sig;
    if signature.asyncness.is_none() {
        return Err(syn::Error::new(
            signature.fn_token.span(),
            "#[inference::tool] takes an `async fn`",
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            signature.generics.span(),
            "a tool function takes no generic parameters",
        ));
    }
    if let Some(variadic) = &signature.variadic {
        return Err(syn::Error::new(
            variadic.span(),
            "a tool function is not variadic",
        ));
    }
    let arguments = arguments(&function)?;
    let name = &signature.ident;
    let tool_name = LitStr::new(&name.unraw().to_string(), name.span());
    let description = LitStr::new(&description(&function.attrs), Span::call_site());
    let visibility = &function.vis;
    let docs: Vec<&Attribute> = function
        .attrs
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .collect();
    let effect = format_ident!("{}", options.effect.unwrap_or("ReadOnly"));
    let confirm = options.confirm;

    let fields = arguments
        .iter()
        .map(|Argument { name, ty }| quote_spanned!(ty.span()=> #name: #ty));
    let names: Vec<&syn::Ident> = arguments.iter().map(|argument| &argument.name).collect();

    let mut body = function.clone();
    body.sig.ident = format_ident!("__inference_tool_body");
    body.vis = syn::Visibility::Inherited;
    body.attrs
        .retain(|attribute| !attribute.path().is_ident("doc"));

    let failure = quote! {
        |detail: ::std::string::String| ::inference::tools::ToolFailure {
            kind: ::inference::tools::ToolFailureKind::ExecutionError,
            detail,
        }
    };
    let answer = if returns_result(&signature.output) {
        quote! {
            match __inference_tool_body(#(arguments.#names),*).await {
                ::core::result::Result::Ok(value) => value,
                ::core::result::Result::Err(error) => {
                    return ::core::result::Result::Err(failure(::std::string::ToString::to_string(&error)));
                }
            }
        }
    } else {
        quote! { __inference_tool_body(#(arguments.#names),*).await }
    };

    Ok(quote! {
        #(#docs)*
        #visibility fn #name() -> (
            ::inference::tools::Tool,
            ::std::boxed::Box<dyn ::inference::tools::ToolExecutor>,
        ) {
            #[derive(::inference::__private::serde::Deserialize, ::inference::__private::schemars::JsonSchema)]
            #[serde(crate = "::inference::__private::serde")]
            #[schemars(crate = "::inference::__private::schemars")]
            struct __InferenceToolArguments {
                #(#fields,)*
            }

            #body

            struct __InferenceToolExecutor;

            impl ::inference::tools::ToolExecutor for __InferenceToolExecutor {
                fn execute<'a>(
                    &'a self,
                    call: &'a ::inference::tools::ToolCall,
                    deadline: ::std::time::Instant,
                    cancellation: &'a ::inference::tools::CancellationSignal,
                ) -> ::inference::tools::ExecutionFuture<'a> {
                    ::std::boxed::Box::pin(async move {
                        let failure = #failure;
                        if cancellation.is_cancelled() {
                            return ::core::result::Result::Err(::inference::tools::ToolFailure {
                                kind: ::inference::tools::ToolFailureKind::Cancelled,
                                detail: ::std::string::String::from("the call was cancelled before it started"),
                            });
                        }
                        if ::std::time::Instant::now() >= deadline {
                            return ::core::result::Result::Err(::inference::tools::ToolFailure {
                                kind: ::inference::tools::ToolFailureKind::DeadlineExceeded,
                                detail: ::std::string::String::from("the deadline passed before the call started"),
                            });
                        }
                        let arguments: __InferenceToolArguments =
                            match ::inference::__private::serde_json::from_value(
                                ::core::clone::Clone::clone(call.arguments()),
                            ) {
                                ::core::result::Result::Ok(arguments) => arguments,
                                ::core::result::Result::Err(error) => {
                                    return ::core::result::Result::Err(failure(::std::format!(
                                        "the arguments of {} do not read: {}",
                                        #tool_name,
                                        error
                                    )));
                                }
                            };
                        let value = #answer;
                        ::inference::__private::serde_json::to_value(value).map_err(|error| {
                            failure(::std::format!(
                                "the result of {} does not write as JSON: {}",
                                #tool_name,
                                error
                            ))
                        })
                    })
                }
            }

            let mut schema = ::inference::__private::schemars::generate::SchemaSettings::draft2020_12()
                .with(|settings| {
                    settings.meta_schema = ::core::option::Option::None;
                    settings.inline_subschemas = true;
                })
                .into_generator()
                .into_root_schema_for::<__InferenceToolArguments>();
            schema.remove("title");
            let mut schema = schema.to_value();
            if let ::core::option::Option::Some(keywords) = schema.as_object_mut() {
                keywords
                    .entry("properties")
                    .or_insert_with(|| ::inference::__private::serde_json::Value::Object(
                        ::inference::__private::serde_json::Map::new(),
                    ));
            }
            let (tool, _) = ::inference::tools::Tool::declare(::inference::tools::DeclareTool {
                name: ::std::string::String::from(#tool_name),
                description: ::std::string::String::from(#description),
                schema,
                schema_revision: ::std::string::String::from("1"),
                side_effect: ::inference::tools::SideEffectClass::#effect,
                requires_confirmation: #confirm,
            });
            (tool, ::std::boxed::Box::new(__InferenceToolExecutor))
        }
    })
}

/// The one signature `#[inference::main]` takes.
const MAIN_SIGNATURE: &str = "#[inference::main] takes `async fn main(host: impl AudioHost, stack: Stack)`, with no return type";

fn expand_main(attribute: &TokenStream2, function: ItemFn) -> syn::Result<TokenStream2> {
    if !attribute.is_empty() {
        return Err(syn::Error::new(
            attribute.span(),
            "#[inference::main] takes no options",
        ));
    }
    let signature = &function.sig;
    let refuse = |span: Span| syn::Error::new(span, MAIN_SIGNATURE);
    if signature.ident != "main"
        || signature.asyncness.is_none()
        || !signature.generics.params.is_empty()
        || signature.generics.where_clause.is_some()
        || signature.variadic.is_some()
        || !matches!(signature.output, ReturnType::Default)
    {
        return Err(refuse(signature.span()));
    }
    let patterns = signature
        .inputs
        .iter()
        .map(|input| match input {
            FnArg::Typed(typed) => Ok(typed.pat.as_ref()),
            FnArg::Receiver(receiver) => Err(refuse(receiver.span())),
        })
        .collect::<syn::Result<Vec<&Pat>>>()?;
    let [host, stack] = patterns.as_slice() else {
        return Err(refuse(signature.inputs.span()));
    };
    let attributes = &function.attrs;
    let block = &function.block;
    Ok(quote! {
        #(#attributes)*
        #[allow(unreachable_code)]
        pub async fn run(
            #host: impl ::inference::audio_device::AudioHost,
            #stack: ::inference::Stack,
        ) -> ::inference::Result<()> {
            let () = #block;
            ::core::result::Result::Ok(())
        }

        #[allow(dead_code)]
        fn main() -> ::std::process::ExitCode {
            let runtime = match ::inference::__private::tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                ::core::result::Result::Ok(runtime) => runtime,
                ::core::result::Result::Err(error) => {
                    return ::inference::__private::exit(::core::result::Result::Err(
                        ::inference::Error::Layer {
                            message: ::std::string::String::from("the async runtime did not start"),
                            source: ::core::option::Option::Some(::std::boxed::Box::new(error)),
                        },
                    ));
                }
            };
            ::inference::__private::exit(runtime.block_on(async {
                run(::inference::audio_device::CpalHost, ::inference::Stack::from_env()?).await
            }))
        }
    })
}
