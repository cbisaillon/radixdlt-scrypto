use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::*;

use crate::utils::*;

macro_rules! trace {
    ($($arg:expr),*) => {{
        #[cfg(feature = "trace")]
        println!($($arg),*);
    }};
}

pub fn handle_encode(
    input: TokenStream,
    context_custom_value_kind: Option<&'static str>,
) -> Result<TokenStream> {
    trace!("handle_encode() starts");

    let parsed: DeriveInput = parse2(input)?;

    let output = match get_derive_strategy(&parsed.attrs)? {
        DeriveStrategy::Normal => handle_normal_encode(parsed, context_custom_value_kind)?,
        DeriveStrategy::Transparent => {
            handle_transparent_encode(parsed, context_custom_value_kind)?
        }
        DeriveStrategy::DeriveAs {
            as_type, as_ref, ..
        } => handle_encode_as(parsed, context_custom_value_kind, &as_type, &as_ref)?,
    };

    #[cfg(feature = "trace")]
    crate::utils::print_generated_code("Encode", &output);

    trace!("handle_encode() finishes");
    Ok(output)
}

pub fn handle_transparent_encode(
    parsed: DeriveInput,
    context_custom_value_kind: Option<&'static str>,
) -> Result<TokenStream> {
    let output = match &parsed.data {
        Data::Struct(s) => {
            let FieldsData {
                unskipped_field_types,
                unskipped_field_names,
                ..
            } = process_fields(&s.fields)?;
            if unskipped_field_types.len() != 1 {
                return Err(Error::new(Span::call_site(), "The transparent attribute is only supported for structs with a single unskipped field."));
            }
            let field_type = &unskipped_field_types[0];
            let field_name = &unskipped_field_names[0];
            handle_encode_as(
                parsed,
                context_custom_value_kind,
                &field_type,
                &quote! { &self.#field_name },
            )?
        }
        Data::Enum(_) => {
            return Err(Error::new(Span::call_site(), "The transparent attribute is only supported for structs with a single unskipped field."));
        }
        Data::Union(_) => {
            return Err(Error::new(Span::call_site(), "Union is not supported!"));
        }
    };

    Ok(output)
}

pub fn handle_encode_as(
    parsed: DeriveInput,
    context_custom_value_kind: Option<&'static str>,
    as_type: &Type,
    as_ref_code: &TokenStream,
) -> Result<TokenStream> {
    let DeriveInput {
        attrs,
        ident,
        generics,
        ..
    } = parsed;
    let (impl_generics, ty_generics, where_clause, custom_value_kind_generic, encoder_generic) =
        build_encode_generics(&generics, &attrs, context_custom_value_kind)?;

    // NOTE: The `: &#as_type` is not strictly needed for the code to compile,
    // but it is useful to sanity check that the user has provided the correct implementation.
    // If they have not, they should get a nice and clear error message.
    let output = quote! {
        impl #impl_generics sbor::Encode <#custom_value_kind_generic, #encoder_generic> for #ident #ty_generics #where_clause {
            #[inline]
            fn encode_value_kind(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                use sbor::{self, Encode};
                let as_ref: &#as_type = #as_ref_code;
                as_ref.encode_value_kind(encoder)
            }

            #[inline]
            fn encode_body(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                use sbor::{self, Encode};
                let as_ref: &#as_type = #as_ref_code;
                as_ref.encode_body(encoder)
            }
        }
    };

    Ok(output)
}

pub fn handle_normal_encode(
    parsed: DeriveInput,
    context_custom_value_kind: Option<&'static str>,
) -> Result<TokenStream> {
    let DeriveInput {
        attrs,
        ident,
        data,
        generics,
        ..
    } = parsed;
    let (impl_generics, ty_generics, where_clause, custom_value_kind_generic, encoder_generic) =
        build_encode_generics(&generics, &attrs, context_custom_value_kind)?;

    let output = match data {
        Data::Struct(s) => {
            let FieldsData {
                unskipped_field_names,
                unskipped_field_count,
                ..
            } = process_fields(&s.fields)?;
            quote! {
                impl #impl_generics sbor::Encode <#custom_value_kind_generic, #encoder_generic> for #ident #ty_generics #where_clause {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(#unskipped_field_count)?;
                        #(encoder.encode(&self.#unskipped_field_names)?;)*
                        Ok(())
                    }
                }
            }
        }
        Data::Enum(DataEnum { variants, .. }) => {
            let EnumVariantsData {
                source_variants, ..
            } = process_enum_variants(&attrs, &variants)?;
            let match_arms = source_variants
                .iter()
                .map(|source_variant| {
                    Ok(match source_variant {
                        SourceVariantData::Reachable(VariantData {
                            source_variant,
                            discriminator,
                            fields_data,
                            ..
                        }) => {
                            let v_id = &source_variant.ident;
                            let FieldsData {
                                unskipped_field_count,
                                fields_unpacking,
                                unskipped_unpacked_field_names,
                                ..
                            } = fields_data;
                            quote! {
                                Self::#v_id #fields_unpacking => {
                                    encoder.write_discriminator(#discriminator)?;
                                    encoder.write_size(#unskipped_field_count)?;
                                    #(encoder.encode(#unskipped_unpacked_field_names)?;)*
                                }
                            }
                        }
                        SourceVariantData::Unreachable(UnreachableVariantData {
                            source_variant,
                            fields_data,
                            ..
                        }) => {
                            let v_id = &source_variant.ident;
                            let FieldsData {
                                empty_fields_unpacking,
                                ..
                            } = &fields_data;
                            let panic_message =
                                format!("Variant {} ignored as unreachable", v_id.to_string());
                            quote! {
                                Self::#v_id #empty_fields_unpacking => panic!(#panic_message),
                            }
                        }
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            let encode_content = if match_arms.len() == 0 {
                quote! {}
            } else {
                quote! {
                    use sbor::{self, Encode};

                    match self {
                        #(#match_arms)*
                    }
                }
            };
            quote! {
                impl #impl_generics sbor::Encode <#custom_value_kind_generic, #encoder_generic> for #ident #ty_generics #where_clause {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Enum)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut #encoder_generic) -> Result<(), sbor::EncodeError> {
                        #encode_content
                        Ok(())
                    }
                }
            }
        }
        Data::Union(_) => {
            return Err(Error::new(Span::call_site(), "Union is not supported!"));
        }
    };

    #[cfg(feature = "trace")]
    crate::utils::print_generated_code("Encode", &output);

    trace!("handle_encode() finishes");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use proc_macro2::TokenStream;
    use std::str::FromStr;

    use super::*;

    fn assert_code_eq(a: TokenStream, b: TokenStream) {
        assert_eq!(a.to_string(), b.to_string());
    }

    #[test]
    fn test_encode_struct() {
        let input = TokenStream::from_str("struct Test {a: u32}").unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <E: sbor::Encoder<X>, X: sbor::CustomValueKind > sbor::Encode<X, E> for Test {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(1)?;
                        encoder.encode(&self.a)?;
                        Ok(())
                    }
                }
            },
        );
    }

    #[test]
    fn test_encode_enum() {
        let input = TokenStream::from_str("enum Test {A, B (u32), C {x: u8}}").unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <E: sbor::Encoder<X>, X: sbor::CustomValueKind > sbor::Encode<X, E> for Test {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Enum)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        match self {
                            Self::A => {
                                encoder.write_discriminator(0u8)?;
                                encoder.write_size(0)?;
                            }
                            Self::B(a0) => {
                                encoder.write_discriminator(1u8)?;
                                encoder.write_size(1)?;
                                encoder.encode(a0)?;
                            }
                            Self::C { x, .. } => {
                                encoder.write_discriminator(2u8)?;
                                encoder.write_size(1)?;
                                encoder.encode(x)?;
                            }
                        }
                        Ok(())
                    }
                }
            },
        );
    }

    #[test]
    fn test_skip() {
        let input = TokenStream::from_str("struct Test {#[sbor(skip)] a: u32}").unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <E: sbor::Encoder<X>, X: sbor::CustomValueKind > sbor::Encode<X, E> for Test {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(0)?;
                        Ok(())
                    }
                }
            },
        );
    }

    #[test]
    fn test_encode_generic() {
        let input = TokenStream::from_str("struct Test<T, E: Clashing> { a: T, b: E, }").unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <T, E: Clashing, E0: sbor::Encoder<X>, X: sbor::CustomValueKind > sbor::Encode<X, E0> for Test<T, E >
                where
                    T: sbor::Encode<X, E0>,
                    E: sbor::Encode<X, E0>
                {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E0) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E0) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(2)?;
                        encoder.encode(&self.a)?;
                        encoder.encode(&self.b)?;
                        Ok(())
                    }
                }
            },
        );
    }

    #[test]
    fn test_encode_struct_with_custom_value_kind() {
        let input = TokenStream::from_str(
            "#[sbor(custom_value_kind = \"NoCustomValueKind\")] struct Test {#[sbor(skip)] a: u32}",
        )
        .unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <E: sbor::Encoder<NoCustomValueKind> > sbor::Encode<NoCustomValueKind, E> for Test {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(0)?;
                        Ok(())
                    }
                }
            },
        );
    }

    #[test]
    fn test_custom_value_kind_canonical_path() {
        let input = TokenStream::from_str(
            "#[sbor(custom_value_kind = \"sbor::basic::NoCustomValueKind\")] struct Test {#[sbor(skip)] a: u32}",
        )
        .unwrap();
        let output = handle_encode(input, None).unwrap();

        assert_code_eq(
            output,
            quote! {
                impl <E: sbor::Encoder<sbor::basic::NoCustomValueKind> > sbor::Encode<sbor::basic::NoCustomValueKind, E> for Test {
                    #[inline]
                    fn encode_value_kind(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        encoder.write_value_kind(sbor::ValueKind::Tuple)
                    }

                    #[inline]
                    fn encode_body(&self, encoder: &mut E) -> Result<(), sbor::EncodeError> {
                        use sbor::{self, Encode};
                        encoder.write_size(0)?;
                        Ok(())
                    }
                }
            },
        );
    }
}
