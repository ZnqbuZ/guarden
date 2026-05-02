use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::token::Bracket;
use syn::{Error, Result, bracketed, parenthesized, parse2};
use syn::{Expr, Ident, Token};

mod kw {
    use syn::custom_keyword;

    custom_keyword!(stmt);
    custom_keyword!(expr);

    custom_keyword!(sync);

    custom_keyword!(export);
    custom_keyword!(all);
    custom_keyword!(wrapped);

    macro_rules! parse {
        ($input:ident; $($kw:ident => $variant:expr),+ $(,)?) => {
            let lookahead = $input.lookahead1();
            $(
                if lookahead.peek(kw::$kw) {
                    $input.parse::<kw::$kw>()?;
                    return Ok($variant);
                }
            )+
            return Err(lookahead.error());
        }
    }

    pub(crate) use parse;
}

enum Action {
    Stmt((Option<Token![mut]>, Ident)),
    Expr,
}

impl Parse for Action {
    fn parse(input: ParseStream) -> Result<Self> {
        let lookahead = input.lookahead1();

        if lookahead.peek(kw::stmt) {
            input.parse::<kw::stmt>()?;
            input.parse::<Token![;]>()?;

            let mut_token = input.parse()?;

            let guard = if input.peek(Ident) && input.peek2(Token![=>]) {
                let ident: Ident = input.parse()?;
                input.parse::<Token![=>]>()?;
                ident
            } else {
                format_ident!("__guard")
            };

            return Ok(Action::Stmt((mut_token, guard)));
        }

        if lookahead.peek(kw::expr) {
            input.parse::<kw::expr>()?;
            input.parse::<Token![;]>()?;

            return Ok(Action::Expr);
        }

        Err(lookahead.error())
    }
}

enum Export {
    All,
    Wrapped,
    Default,
}

impl Parse for Export {
    fn parse(input: ParseStream) -> Result<Self> {
        kw::parse! {
            input;
            all => Export::All,
            wrapped => Export::Wrapped,
        }
    }
}

struct Arg {
    mut_token: Option<Token![mut]>,
    ident: Ident,
    init: Option<Expr>,
}

impl Parse for Arg {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut_token = input.parse()?;
        let ident: Ident = input.parse()?;
        let init = input
            .parse::<Option<Token![=]>>()?
            .map(|_| input.parse())
            .transpose()?;
        Ok(Arg {
            mut_token,
            ident,
            init,
        })
    }
}

struct MacroInput {
    action: Action,
    sync_token: Option<kw::sync>,
    move_token: Option<Token![move]>,
    export: Export,
    args: Vec<Arg>,
    body: TokenStream,
}

impl Parse for MacroInput {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![@]>()?;
        let action = input.parse()?;

        let sync_token = input.parse()?;
        let move_token = input.parse()?;

        let export = if let Some(kw) = input.parse::<Option<kw::export>>()? {
            let export;
            parenthesized!(export in input);
            let export = export.parse()?;

            if matches!(action, Action::Expr) && !matches!(export, Export::Wrapped) {
                return Err(Error::new_spanned(
                    kw,
                    "Only `export(wrapped)` is allowed in expression mode",
                ));
            }

            export
        } else {
            Export::Default
        };

        let args = if input.peek(Bracket) {
            let args;
            bracketed!(args in input);
            args.parse_terminated(Arg::parse, Token![,])?
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };

        let body = input.parse()?;

        Ok(MacroInput {
            action,
            sync_token,
            move_token,
            export,
            args,
            body,
        })
    }
}

pub fn proc(input: TokenStream) -> Result<TokenStream> {
    let input = parse2::<MacroInput>(input)?;

    #[allow(non_snake_case)]
    let (SYNC, ASYNC) = if input.sync_token.is_some() {
        (quote!(true), quote!(false))
    } else {
        (quote!(false), quote!(_))
    };

    let mut inits = Vec::new();
    let mut vars = Vec::new();
    let mut fields = Vec::new();
    let mut exports = Vec::new();

    let args = input.args;
    for arg in &args {
        let ident = &arg.ident;

        let init = if let Some(expr) = arg.init.clone() {
            quote!(#expr)
        } else {
            quote!(#ident)
        };
        inits.push(init);

        let mut_token = arg.mut_token;
        vars.push(quote!(#mut_token #ident));
        exports.push(quote!(ref #mut_token #ident));

        fields.push(ident.clone());
    }

    let export = input.export;

    let guard_expr = {
        let move_token = input.move_token;
        let body = input.body;
        let new = quote!(guarden::guard::ContextGuard::<#SYNC, #ASYNC, _, _>::new);

        match export {
            Export::Wrapped => {
                let context = format_ident!("__gCtx");
                let generics = fields
                    .iter()
                    .enumerate()
                    .map(|(i, _)| format_ident!("{}FieldType{}", context, i))
                    .collect::<Vec<_>>();
                quote! {
                    {
                        #[derive(Debug)]
                        #[allow(non_snake_case, non_camel_case_types)]
                        struct #context <#(#generics),*> {
                            #(#fields: #generics),*
                        }
                        #[allow(non_shorthand_field_patterns)]
                        #new(
                            #context { #(#fields: #inits),* },
                            #move_token | #context { #(#fields: #vars),* } | { #body }
                        )
                    }
                }
            }
            _ => {
                // NOTE: When there is exactly 1 capture (e.g., `[mut a]`), `#(#inits),*` expands to `(a)`,
                // which is NOT a tuple but a parenthesized scalar expression.
                quote! {
                    #new(
                        (#(#inits),*),
                        #move_token |#[allow(unused_parens, unused_mut)] (#(#vars),*)| { #body }
                    )
                }
            }
        }
    };

    let output = match input.action {
        Action::Expr => guard_expr,
        Action::Stmt((mut_token, guard)) => {
            let allow_unused_mut = mut_token.is_none().then(|| quote!(#[allow(unused_mut)]));
            let mut_token = mut_token.or_else(|| {
                match export {
                    Export::All => args.iter().any(|arg| arg.mut_token.is_some()),
                    Export::Default => args
                        .iter()
                        .any(|arg| arg.mut_token.is_some() && arg.init.is_none()),
                    _ => false,
                }
                .then_some(<Token![mut]>::default())
            });

            match export {
                Export::Wrapped => {
                    quote! {
                        #allow_unused_mut
                        let #mut_token #guard = #guard_expr;
                    }
                }
                Export::All | Export::Default => {
                    let exports = match export {
                        Export::All => quote!(#(#exports),*),
                        Export::Default => {
                            let exports = exports.iter().zip(&args).map(|(export, arg)| {
                                if arg.init.is_none() {
                                    quote!(#export)
                                } else {
                                    quote!(_)
                                }
                            });
                            quote!(#(#exports),*)
                        }
                        _ => unreachable!(),
                    };

                    quote! {
                        let #mut_token #guard = #guard_expr;

                        #[allow(unused_parens, unused_variables, clippy::toplevel_ref_arg)]
                        let (#exports) = *#guard;
                    }
                }
            }
        }
    };

    Ok(output)
}
