use darling::ast::Data;
use proc_macro::TokenStream;
use quote::quote;
use syn::ext::IdentExt;
use syn::Error;

use crate::args::{self, RenameRuleExt, RenameTarget};
use crate::utils::{
    generate_default, generate_validator, get_crate_name, get_rustdoc, GeneratorResult,
};

pub fn generate(object_args: &args::InputObject) -> GeneratorResult<TokenStream> {
    let crate_name = get_crate_name(object_args.internal);
    let ident = &object_args.ident;
    let s = match &object_args.data {
        Data::Struct(s) => s,
        _ => {
            return Err(
                Error::new_spanned(ident, "InputObject can only be applied to an struct.").into(),
            )
        }
    };

    let mut struct_fields = Vec::new();
    for field in &s.fields {
        let vis = &field.vis;
        let ty = &field.ty;
        let ident = &field.ident;
        let attrs = field
            .attrs
            .iter()
            .filter(|attr| !attr.path.is_ident("field"))
            .collect::<Vec<_>>();
        struct_fields.push(quote! {
            #(#attrs)*
            #vis #ident: #ty
        });
    }

    let gql_typename = object_args
        .name
        .clone()
        .unwrap_or_else(|| RenameTarget::Type.rename(ident.to_string()));

    let desc = get_rustdoc(&object_args.attrs)?
        .map(|s| quote! { ::std::option::Option::Some(#s) })
        .unwrap_or_else(|| quote! {::std::option::Option::None});

    let mut get_fields = Vec::new();
    let mut put_fields = Vec::new();
    let mut fields = Vec::new();
    let mut schema_fields = Vec::new();
    let mut flatten_fields = Vec::new();

    for field in &s.fields {
        let ident = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        let name = field.name.clone().unwrap_or_else(|| {
            object_args
                .rename_fields
                .rename(ident.unraw().to_string(), RenameTarget::Field)
        });

        if field.flatten {
            flatten_fields.push((ident, ty));

            schema_fields.push(quote! {
                #crate_name::static_assertions::assert_impl_one!(#ty: #crate_name::InputObjectType);
                #ty::create_type_info(registry);
                if let #crate_name::registry::MetaType::InputObject { input_fields, .. } =
                    registry.create_dummy_type::<#ty>() {
                    fields.extend(input_fields);
                }
            });

            get_fields.push(quote! {
                let #ident: #ty = #crate_name::InputValueType::parse(
                    ::std::option::Option::Some(#crate_name::Value::Object(::std::clone::Clone::clone(&obj)))
                ).map_err(#crate_name::InputValueError::propagate)?;
            });

            fields.push(ident);

            put_fields.push(quote! {
                if let #crate_name::Value::Object(values) = #crate_name::InputValueType::to_value(&self.#ident) {
                    map.extend(values);
                }
            });
            continue;
        }

        let validator = match &field.validator {
            Some(meta) => {
                let stream = generate_validator(&crate_name, meta)?;
                quote!(::std::option::Option::Some(#stream))
            }
            None => quote!(::std::option::Option::None),
        };
        let desc = get_rustdoc(&field.attrs)?
            .map(|s| quote! { ::std::option::Option::Some(#s) })
            .unwrap_or_else(|| quote! {::std::option::Option::None});
        let default = generate_default(&field.default, &field.default_with)?;
        let schema_default = default
            .as_ref()
            .map(|value| {
                quote! {
                    ::std::option::Option::Some(::std::string::ToString::to_string(
                        &<#ty as #crate_name::InputValueType>::to_value(&#value)
                    ))
                }
            })
            .unwrap_or_else(|| quote!(::std::option::Option::None));

        if let Some(default) = default {
            get_fields.push(quote! {
                #[allow(non_snake_case)]
                let #ident: #ty = {
                    match obj.get(#name) {
                        ::std::option::Option::Some(value) => {
                            #crate_name::InputValueType::parse(::std::option::Option::Some(::std::clone::Clone::clone(&value)))
                                .map_err(#crate_name::InputValueError::propagate)?
                        },
                        ::std::option::Option::None => #default,
                    }
                };
            });
        } else {
            get_fields.push(quote! {
                #[allow(non_snake_case)]
                let #ident: #ty = #crate_name::InputValueType::parse(obj.get(#name).cloned())
                    .map_err(#crate_name::InputValueError::propagate)?;
            });
        }

        put_fields.push(quote! {
            map.insert(
                #crate_name::Name::new(#name),
                #crate_name::InputValueType::to_value(&self.#ident)
            );
        });

        fields.push(ident);
        schema_fields.push(quote! {
            fields.insert(::std::borrow::ToOwned::to_owned(#name), #crate_name::registry::MetaInputValue {
                name: #name,
                description: #desc,
                ty: <#ty as #crate_name::Type>::create_type_info(registry),
                default_value: #schema_default,
                validator: #validator,
            });
        })
    }

    if get_fields.is_empty() {
        return Err(Error::new_spanned(
            &ident,
            "An GraphQL Input Object type must define one or more input fields.",
        )
        .into());
    }

    let expanded = quote! {
        #[allow(clippy::all, clippy::pedantic)]
        impl #crate_name::Type for #ident {
            fn type_name() -> ::std::borrow::Cow<'static, ::std::primitive::str> {
                ::std::borrow::Cow::Borrowed(#gql_typename)
            }

            fn create_type_info(registry: &mut #crate_name::registry::Registry) -> ::std::string::String {
                registry.create_type::<Self, _>(|registry| #crate_name::registry::MetaType::InputObject {
                    name: ::std::borrow::ToOwned::to_owned(#gql_typename),
                    description: #desc,
                    input_fields: {
                        let mut fields = #crate_name::indexmap::IndexMap::new();
                        #(#schema_fields)*
                        fields
                    }
                })
            }
        }

        #[allow(clippy::all, clippy::pedantic)]
        impl #crate_name::InputValueType for #ident {
            fn parse(value: ::std::option::Option<#crate_name::Value>) -> #crate_name::InputValueResult<Self> {
                if let ::std::option::Option::Some(#crate_name::Value::Object(obj)) = value {
                    #(#get_fields)*
                    ::std::result::Result::Ok(Self { #(#fields),* })
                } else {
                    ::std::result::Result::Err(#crate_name::InputValueError::expected_type(value.unwrap_or_default()))
                }
            }

            fn to_value(&self) -> #crate_name::Value {
                let mut map = ::std::collections::BTreeMap::new();
                #(#put_fields)*
                #crate_name::Value::Object(map)
            }
        }

        impl #crate_name::InputObjectType for #ident {}
    };
    Ok(expanded.into())
}
