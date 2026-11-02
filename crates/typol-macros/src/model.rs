use crate::attributes::Attributes;
use quote::ToTokens;
use syn::{
    Data, DeriveInput, Error, Fields, GenericArgument, GenericParam, Ident, LitStr, Path,
    PathArguments, Result, Type, TypeParamBound, parse2,
};

pub(crate) struct FieldModel {
    pub(crate) ident: Ident,
    pub(crate) name: LitStr,
    pub(crate) ty: Type,
}

pub(crate) enum PayloadModel {
    Unit,
    Single(Box<Type>),
    Structure(Vec<FieldModel>),
}

pub(crate) struct VariantModel {
    pub(crate) ident: Ident,
    pub(crate) name: LitStr,
    pub(crate) payload: PayloadModel,
}

pub(crate) enum StructureForm {
    Unit,
    Named,
}

pub(crate) enum BodyModel {
    Structure {
        fields: Vec<FieldModel>,
        form: StructureForm,
    },
    Enumeration(Vec<VariantModel>),
}

pub(crate) struct Model {
    input: DeriveInput,
    name: LitStr,
    facade: Path,
    trusted: bool,
    body: BodyModel,
}

impl Model {
    pub(crate) fn parse(input: DeriveInput) -> Result<Self> {
        let attributes = Attributes::parse(&input.attrs, true)?;
        for parameter in &input.generics.params {
            match parameter {
                GenericParam::Lifetime(parameter) => {
                    return Err(Error::new_spanned(
                        parameter,
                        "lifetime generics are not supported",
                    ));
                }
                GenericParam::Type(parameter) => Attributes::reject(&parameter.attrs)?,
                GenericParam::Const(parameter) => Attributes::reject(&parameter.attrs)?,
            }
        }
        let body = match &input.data {
            Data::Struct(data) => match &data.fields {
                Fields::Unit => BodyModel::Structure {
                    fields: Vec::new(),
                    form: StructureForm::Unit,
                },
                Fields::Named(_) => BodyModel::Structure {
                    fields: Self::fields(&data.fields)?,
                    form: StructureForm::Named,
                },
                Fields::Unnamed(fields) => {
                    return Err(Error::new_spanned(
                        fields,
                        "tuple structs are not supported; only named-field structs and unit structs are supported",
                    ));
                }
            },
            Data::Enum(data) => {
                if data.variants.is_empty() {
                    return Err(Error::new_spanned(
                        &input.ident,
                        "empty enums are not supported",
                    ));
                }
                let mut variants = Vec::new();
                for variant in &data.variants {
                    let attributes = Attributes::parse(&variant.attrs, false)?;
                    let payload = match &variant.fields {
                        Fields::Unit => PayloadModel::Unit,
                        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                            let field = &fields.unnamed[0];
                            Attributes::reject(&field.attrs)?;
                            Self::validate_type(&field.ty)?;
                            PayloadModel::Single(Box::new(parse2(field.ty.to_token_stream())?))
                        }
                        Fields::Named(_) => PayloadModel::Structure(Self::fields(&variant.fields)?),
                        Fields::Unnamed(fields) => {
                            return Err(Error::new_spanned(
                                fields,
                                "tuple variants must have exactly one payload",
                            ));
                        }
                    };
                    variants.push(VariantModel {
                        ident: variant.ident.clone(),
                        name: attributes.name(&variant.ident)?,
                        payload,
                    });
                }
                Attributes::unique(variants.iter().map(|variant| &variant.name))?;
                BodyModel::Enumeration(variants)
            }
            Data::Union(data) => {
                return Err(Error::new(
                    data.union_token.span,
                    "unions are not supported",
                ));
            }
        };
        Ok(Self {
            name: attributes.name(&input.ident)?,
            facade: attributes.facade()?,
            trusted: attributes.trusted(),
            input,
            body,
        })
    }

    pub(crate) fn input(&self) -> &DeriveInput {
        &self.input
    }

    pub(crate) fn name(&self) -> &LitStr {
        &self.name
    }

    pub(crate) fn facade(&self) -> &Path {
        &self.facade
    }

    pub(crate) const fn trusted(&self) -> bool {
        self.trusted
    }

    pub(crate) fn body(&self) -> &BodyModel {
        &self.body
    }

    pub(crate) fn types(&self) -> Vec<&Type> {
        match &self.body {
            BodyModel::Structure { fields, .. } => fields.iter().map(|field| &field.ty).collect(),
            BodyModel::Enumeration(variants) => variants
                .iter()
                .flat_map(|variant| match &variant.payload {
                    PayloadModel::Unit => Vec::new(),
                    PayloadModel::Single(ty) => vec![ty.as_ref()],
                    PayloadModel::Structure(fields) => {
                        fields.iter().map(|field| &field.ty).collect()
                    }
                })
                .collect(),
        }
    }

    fn fields(fields: &Fields) -> Result<Vec<FieldModel>> {
        let Fields::Named(fields) = fields else {
            return Err(Error::new_spanned(
                fields,
                "only named-field structs are supported",
            ));
        };
        let mut result = Vec::new();
        for field in &fields.named {
            let attributes = Attributes::parse(&field.attrs, false)?;
            Self::validate_type(&field.ty)?;
            let Some(ident) = field.ident.clone() else {
                return Err(Error::new_spanned(
                    field,
                    "named field is missing an identifier",
                ));
            };
            result.push(FieldModel {
                name: attributes.name(&ident)?,
                ident,
                ty: parse2(field.ty.to_token_stream())?,
            });
        }
        Attributes::unique(result.iter().map(|field| &field.name))?;
        Ok(result)
    }

    fn validate_type(ty: &Type) -> Result<()> {
        match ty {
            Type::Path(path) => {
                if let Some(qself) = &path.qself {
                    Self::validate_type(&qself.ty)?;
                }
                for segment in &path.path.segments {
                    match &segment.arguments {
                        PathArguments::AngleBracketed(arguments) => {
                            for argument in &arguments.args {
                                match argument {
                                    GenericArgument::Type(ty) => Self::validate_type(ty)?,
                                    GenericArgument::Lifetime(lifetime) => {
                                        return Err(Error::new_spanned(
                                            lifetime,
                                            "borrowed targets are not supported",
                                        ));
                                    }
                                    GenericArgument::AssocType(binding) => {
                                        Self::validate_type(&binding.ty)?
                                    }
                                    GenericArgument::Constraint(binding) => {
                                        for bound in &binding.bounds {
                                            if let TypeParamBound::Lifetime(lifetime) = bound {
                                                return Err(Error::new_spanned(
                                                    lifetime,
                                                    "borrowed targets are not supported",
                                                ));
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        PathArguments::Parenthesized(arguments) => {
                            return Err(Error::new_spanned(
                                arguments,
                                "function type targets are not supported",
                            ));
                        }
                        PathArguments::None => {}
                    }
                }
                Ok(())
            }
            Type::Array(array) => Self::validate_type(&array.elem),
            Type::Paren(inner) => Self::validate_type(&inner.elem),
            Type::Group(inner) => Self::validate_type(&inner.elem),
            _ => Err(Error::new_spanned(
                ty,
                "unsupported type syntax; the target must own its data",
            )),
        }
    }
}
