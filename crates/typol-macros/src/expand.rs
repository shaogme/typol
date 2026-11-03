use crate::model::{BodyModel, FieldModel, Model, PayloadModel, StructureForm, VariantModel};
use proc_macro2::{Ident, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote};
use std::collections::HashSet;
use syn::{
    GenericArgument, GenericParam, Generics, PathArguments, Result, Type, TypePath, parse2,
    visit::{self, Visit},
};

struct ExpansionBody {
    describe: TokenStream,
    encode: TokenStream,
    decode: TokenStream,
}

struct AssociatedTypeCollector<'a> {
    parameters: &'a [Ident],
    projections: Vec<TokenStream>,
    seen: HashSet<String>,
}

impl<'ast> Visit<'ast> for AssociatedTypeCollector<'_> {
    fn visit_type_path(&mut self, path: &'ast TypePath) {
        let parameter_projection = path.path.segments.len() > 1
            && path
                .path
                .segments
                .first()
                .is_some_and(|segment| self.parameters.contains(&segment.ident));
        if path.qself.is_some() || parameter_projection {
            let tokens = path.to_token_stream();
            let key = tokens.to_string();
            if self.seen.insert(key) {
                self.projections.push(tokens);
            }
        }
        visit::visit_type_path(self, path);
    }
}

pub(crate) struct Expander<'model> {
    model: &'model Model,
    prefix: String,
}

impl<'model> Expander<'model> {
    pub(crate) fn expand(model: &'model Model) -> Result<TokenStream> {
        let input = model.input().to_token_stream();
        let prefix = Self::first_available_prefix(input);
        Self { model, prefix }.tokens()
    }

    fn ident(&self, suffix: &str) -> Ident {
        Ident::new(&format!("{}{suffix}", self.prefix), Span::mixed_site())
    }

    fn first_available_prefix(tokens: TokenStream) -> String {
        let mut occupied = HashSet::new();
        Self::collect_prefix_indices(tokens, &mut occupied);
        let mut index = 0;
        while occupied.contains(&index) {
            index += 1;
        }
        format!("__typol_{index}_")
    }

    fn collect_prefix_indices(tokens: TokenStream, occupied: &mut HashSet<usize>) {
        for token in tokens {
            match token {
                TokenTree::Ident(ident) => {
                    let text = ident.to_string();
                    let Some((digits, _)) = text
                        .strip_prefix("__typol_")
                        .and_then(|suffix| suffix.split_once('_'))
                    else {
                        continue;
                    };
                    if digits.len() > 1 && digits.starts_with('0') {
                        continue;
                    }
                    if digits.bytes().all(|byte| byte.is_ascii_digit()) {
                        if let Ok(index) = digits.parse() {
                            occupied.insert(index);
                        }
                    }
                }
                TokenTree::Group(group) => Self::collect_prefix_indices(group.stream(), occupied),
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
        }
    }

    fn tokens(&self) -> Result<TokenStream> {
        let model = self.model;
        let facade = model.facade();
        let ident = &model.input().ident;
        let name = model.name();
        let trait_name = self.ident("Typol");
        let budgeted_trait_name = self.ident("BudgetedTypol");
        let describe_context = self.ident("DescribeContext");
        let descriptor = self.ident("TypeDescriptor");
        let node = self.ident("DescriptorNode");
        let provider = self.ident("DescriptorProvider");
        let shape = self.ident("TypeShape");
        let variant_shape = self.ident("VariantShape");
        let conversion = self.ident("ConversionContext");
        let errors = self.ident("CodecErrors");
        let value_type = self.ident("ValueId");
        let value_view = self.ident("ConversionValue");
        let payload_ref = self.ident("ConversionPayload");
        let type_id = self.ident("TypeId");
        let context = self.ident("context");
        let value = self.ident("value");
        let members = self.ident("members");
        let result = self.ident("OResult");
        let std_result = self.ident("Result");
        let out_type = self.ident("Out");
        let out = self.ident("out");
        let mut generics: Generics = parse2(model.input().generics.to_token_stream())?;
        if let Some(clause) = &model.input().generics.where_clause {
            generics.where_clause = Some(parse2(clause.to_token_stream())?);
        }
        let types = model.types();
        let parameters: Vec<_> = generics
            .type_params()
            .map(|parameter| parameter.ident.clone())
            .collect();
        let projections = Self::associated_type_projections(&types, &parameters);
        for parameter in &mut generics.params {
            if let GenericParam::Type(parameter) = parameter {
                let prefix = parameter.ident.to_string();
                if types
                    .iter()
                    .any(|ty| Self::uses_type_parameter(ty, &prefix))
                {
                    parameter.bounds.push(parse2(quote!(#trait_name))?);
                }
            }
        }
        for projection in &projections {
            if Self::uses_ident(projection.clone(), "Self")
                || Self::uses_ident(projection.clone(), &ident.to_string())
            {
                continue;
            }
            generics
                .make_where_clause()
                .predicates
                .push(parse2(quote!(#projection: #trait_name))?);
        }
        generics
            .make_where_clause()
            .predicates
            .push(parse2(quote!(Self: 'static))?);
        let split = generics.split_for_impl();
        let impl_generics = split.0;
        let type_generics = split.1;
        let where_clause = split.2;
        let (budgeted_import, budgeted_impl) = if model.trusted() {
            (TokenStream::new(), TokenStream::new())
        } else {
            let mut budgeted_generics: Generics = parse2(model.input().generics.to_token_stream())?;
            if let Some(clause) = &model.input().generics.where_clause {
                budgeted_generics.where_clause = Some(parse2(clause.to_token_stream())?);
            }
            for parameter in &mut budgeted_generics.params {
                if let GenericParam::Type(parameter) = parameter {
                    let prefix = parameter.ident.to_string();
                    if types
                        .iter()
                        .any(|ty| Self::uses_type_parameter(ty, &prefix))
                    {
                        parameter.bounds.push(parse2(quote!(#budgeted_trait_name))?);
                    }
                }
            }
            for projection in &projections {
                if Self::uses_ident(projection.clone(), "Self")
                    || Self::uses_ident(projection.clone(), &ident.to_string())
                {
                    continue;
                }
                budgeted_generics
                    .make_where_clause()
                    .predicates
                    .push(parse2(quote!(#projection: #budgeted_trait_name))?);
            }
            budgeted_generics
                .make_where_clause()
                .predicates
                .push(parse2(quote!(Self: 'static))?);
            let budgeted_split = budgeted_generics.split_for_impl();
            let budgeted_impl_generics = budgeted_split.0;
            let budgeted_type_generics = budgeted_split.1;
            let budgeted_where_clause = budgeted_split.2;
            (
                quote! { BudgetedTypol as #budgeted_trait_name, },
                quote! {
                    impl #budgeted_impl_generics #budgeted_trait_name for #ident #budgeted_type_generics #budgeted_where_clause {}
                },
            )
        };
        let body = match model.body() {
            BodyModel::Structure { fields, form } => {
                let describe = self.describe_fields(fields);
                let encode = self.encode_fields(fields, true);
                let decode = self.decode_fields(fields);
                let construction = match form {
                    StructureForm::Unit => quote!(Self),
                    StructureForm::Named => quote!(Self { #decode }),
                };
                ExpansionBody {
                    describe: quote! { let #members = #describe; #shape::Structure(#members) },
                    encode: quote! { #context.structure(|#value, #context, mut #out| { #encode }, #out.reborrow()) },
                    decode: quote! {
                        let #members = #context.read_structure(#value, #out.reborrow())?;
                        #std_result::Ok(#construction)
                    },
                }
            }
            BodyModel::Enumeration(variants) => self.enumeration(variants),
        };
        let ExpansionBody {
            describe,
            encode,
            decode,
        } = body;
        let provider_import = if model.types().is_empty() {
            TokenStream::new()
        } else {
            quote! { DescriptorProvider as #provider, }
        };
        let enum_imports = match model.body() {
            BodyModel::Enumeration(_) => {
                quote! { VariantShape as #variant_shape, ConversionPayload as #payload_ref, }
            }
            BodyModel::Structure { .. } => TokenStream::new(),
        };
        let option = self.ident("Option");
        Ok(quote! {
            const _: () = {
                use #facade::__private::{
                    Typol as #trait_name, #budgeted_import
                    DescribeContext as #describe_context,
                    TypeDescriptor as #descriptor, DescriptorNode as #node,
                    TypeShape as #shape,
                    ConversionContext as #conversion,
                    CodecErrors as #errors, ValueId as #value_type,
                    ConversionValue as #value_view,
                    OResult as #result, Out as #out_type,
                    #provider_import
                    #enum_imports
                };
                use ::core::{
                    any::TypeId as #type_id,
                    option::Option as #option,
                    result::Result as #std_result,
                };

                impl #impl_generics #trait_name for #ident #type_generics #where_clause {
                    fn describe<'__opeo_slot, '__opeo_borrow>(
                        #context: &mut #describe_context<'_>,
                        mut #out: #out_type<'__opeo_slot, '__opeo_borrow, #errors>,
                    ) -> #result<'__opeo_slot, #descriptor> {
                        let #members = { #describe };
                        let __type_name = #context.name(#name, #out.reborrow())?;
                        #context.insert(#node::Type {
                            identity: #type_id::of::<Self>(),
                            name: #option::Some(__type_name),
                            shape: #members,
                        }, #context.depth(), #out.reborrow())
                    }

                    fn to_config_value<'__opeo_slot, '__opeo_borrow>(
                        &self,
                        #context: &mut #conversion<'_>,
                        mut #out: #out_type<'__opeo_slot, '__opeo_borrow, #errors>,
                    ) -> #result<'__opeo_slot, #value_type> {
                        #context.start::<Self>(#out.reborrow())?;
                        #encode
                    }

                    fn from_config_value<'__opeo_slot, '__opeo_borrow>(
                        #value: #value_view<'_>,
                        #context: &mut #conversion<'_>,
                        mut #out: #out_type<'__opeo_slot, '__opeo_borrow, #errors>,
                    ) -> #result<'__opeo_slot, Self> {
                        #context.start::<Self>(#out.reborrow())?;
                        #decode
                    }
                }

                #budgeted_impl
            };
        })
    }

    fn uses_type_parameter(ty: &Type, name: &str) -> bool {
        match ty {
            Type::Path(path) => {
                let direct = path.qself.is_none()
                    && path.path.leading_colon.is_none()
                    && path
                        .path
                        .segments
                        .first()
                        .is_some_and(|segment| segment.ident == name);
                direct
                    || path
                        .qself
                        .as_ref()
                        .is_some_and(|qself| Self::uses_type_parameter(&qself.ty, name))
                    || path.path.segments.iter().any(|segment| {
                        let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
                            return false;
                        };
                        arguments.args.iter().any(|argument| match argument {
                            GenericArgument::Type(ty) => Self::uses_type_parameter(ty, name),
                            GenericArgument::AssocType(binding) => {
                                Self::uses_type_parameter(&binding.ty, name)
                            }
                            _ => false,
                        })
                    })
            }
            Type::Array(array) => Self::uses_type_parameter(&array.elem, name),
            Type::Paren(inner) => Self::uses_type_parameter(&inner.elem, name),
            Type::Group(inner) => Self::uses_type_parameter(&inner.elem, name),
            _ => false,
        }
    }

    fn associated_type_projections(types: &[&Type], parameters: &[Ident]) -> Vec<TokenStream> {
        let mut collector = AssociatedTypeCollector {
            parameters,
            projections: Vec::new(),
            seen: HashSet::new(),
        };
        for ty in types {
            collector.visit_type(ty);
        }
        collector.projections
    }

    fn uses_ident(tokens: TokenStream, name: &str) -> bool {
        tokens.into_iter().any(|token| match token {
            TokenTree::Ident(ident) => ident == name,
            TokenTree::Group(group) => Self::uses_ident(group.stream(), name),
            TokenTree::Punct(_) | TokenTree::Literal(_) => false,
        })
    }

    fn describe_fields(&self, fields: &[FieldModel]) -> TokenStream {
        let context = self.ident("context");
        let node = self.ident("DescriptorNode");
        let provider = self.ident("DescriptorProvider");
        let value = self.ident("value");
        let out = self.ident("out");
        let members = fields.iter().map(|field| {
            let name = &field.name;
            let ty = &field.ty;
            quote! {
                {
                    let field_name = #context.name(#name, #out.reborrow())?;
                    #context.insert(#node::Field {
                        name: field_name, value: #provider::of::<#ty>(),
                    }, #context.depth(), #out.reborrow())?
                }
            }
        });
        quote! {
            {
                let #value = [ #(#members),* ];
            #context.members(&#value, #out.reborrow())?
            }
        }
    }

    fn encode_fields(&self, fields: &[FieldModel], structure: bool) -> TokenStream {
        let context = self.ident("context");
        let value = self.ident("value");
        let out = self.ident("out");
        let arms = fields.iter().enumerate().map(|(slot, field)| {
            let ident = &field.ident;
            let ty = &field.ty;
            let access = if structure {
                quote!(&self.#ident)
            } else {
                let binding = self.ident(&format!("field_{slot}"));
                quote!(#binding)
            };
            quote! {
                #slot => #context.encode_here::<#ty>(
                    #access, #out.reborrow()
                ),
            }
        });
        quote! { match #value { #(#arms)* _ => #context.reject_with_path(#out.reborrow()), } }
    }

    fn decode_fields(&self, fields: &[FieldModel]) -> TokenStream {
        let context = self.ident("context");
        let members = self.ident("members");
        let out = self.ident("out");
        let fields = fields.iter().enumerate().map(|(slot, field)| {
            let ident = &field.ident;
            let ty = &field.ty;
            quote! {
                #ident: #context.read_field::<#ty>(
                    #members, #slot, #out.reborrow()
                )?,
            }
        });
        quote! { #(#fields)* }
    }

    fn enumeration(&self, variants: &[VariantModel]) -> ExpansionBody {
        let context = self.ident("context");
        let value = self.ident("value");
        let members = self.ident("members");
        let input = self.ident("input");
        let payload = self.ident("payload");
        let node = self.ident("DescriptorNode");
        let provider = self.ident("DescriptorProvider");
        let shape = self.ident("TypeShape");
        let variant_shape = self.ident("VariantShape");
        let payload_ref = self.ident("ConversionPayload");
        let std_result = self.ident("Result");
        let out = self.ident("out");
        let mut descriptors = Vec::new();
        let mut encoders = Vec::new();
        let mut decoders = Vec::new();
        for (slot, variant) in variants.iter().enumerate() {
            let ident = &variant.ident;
            let name = &variant.name;
            let description;
            match &variant.payload {
                PayloadModel::Unit => {
                    description = quote!(#variant_shape::Unit);
                    encoders.push(quote!(Self::#ident => #context.unit_variant(
                            #slot, #out.reborrow()
                        ),));
                    decoders.push(quote! {
                        (#slot, #payload_ref::Unit) => #std_result::Ok(Self::#ident),
                    });
                }
                PayloadModel::Single(ty) => {
                    description = quote!(#variant_shape::Single(#provider::of::<#ty>()));
                    encoders.push(quote! {
                        Self::#ident(#value) => #context.single_variant::<#ty>(
                            #slot, #value, #out.reborrow()
                        ),
                    });
                    decoders.push(quote! {
                        (#slot, #payload_ref::Single(#value)) =>
                            #std_result::Ok(Self::#ident(
                                #context.read_single::<#ty>(#slot, #value, #out.reborrow())?
                            )),
                    });
                }
                PayloadModel::Structure(fields) => {
                    let described = self.describe_fields(fields);
                    description = quote!(#variant_shape::Structure(#described));
                    let bindings = fields.iter().enumerate().map(|(index, field)| {
                        let ident = &field.ident;
                        let binding = self.ident(&format!("field_{index}"));
                        quote!(#ident: #binding)
                    });
                    let encoded = self.encode_fields(fields, false);
                    let decoded = self.decode_fields(fields);
                    encoders.push(quote! {
                        Self::#ident { #(#bindings),* } => {
                            #context.structure_variant(
                                #slot,
                                |#value, #context, mut #out| { #encoded },
                                #out.reborrow()
                            )
                        },
                    });
                    decoders.push(quote! {
                        (#slot, #payload_ref::Structure(#members)) => #context.variant(
                            #slot,
                            |#context, _payload, mut #out| {
                                #std_result::Ok(Self::#ident { #decoded })
                            },
                            #out.reborrow()
                        ),
                    });
                }
            }
            descriptors.push(quote! {
                {
                    let #payload = #description;
                    let variant_name = #context.name(#name, #out.reborrow())?;
                    #context.insert(
                        #node::Variant { name: variant_name, shape: #payload },
                        #context.depth(),
                        #out.reborrow()
                    )?
                }
            });
        }
        ExpansionBody {
            describe: quote! {
                let #members = [ #(#descriptors),* ];
                let #members = #context.members(&#members, #out.reborrow())?;
                #shape::Enumeration(#members)
            },
            encode: quote! { match self { #(#encoders)* } },
            decode: quote! {
                let #input = #context.read_enumeration(#value, #out.reborrow())?;
                match (#input.rust_slot, #input.payload) {
                    #(#decoders)*
                    _ => #context.reject_with_path(#out.reborrow()),
                }
            },
        }
    }
}
