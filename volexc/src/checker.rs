use std::collections::HashMap;

use ariadne::{Color, Label, Report, ReportKind, Source};

use crate::schema::*;

pub struct CheckError {
    pub message: String,
    pub labels: Vec<(Span, String, Color)>,
}

impl CheckError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            labels: Vec::new(),
        }
    }

    fn label(mut self, span: Span, message: impl Into<String>, color: Color) -> Self {
        self.labels.push((span, message.into(), color));
        self
    }
}

struct Checker<'a> {
    schema: &'a Schema,
    errors: Vec<CheckError>,
}

impl<'a> Checker<'a> {
    fn new(schema: &'a Schema) -> Self {
        Self {
            schema,
            errors: Vec::new(),
        }
    }

    fn check(mut self) -> Vec<CheckError> {
        self.check_duplicate_definitions();

        // Check each item
        for item in &self.schema.items {
            match &item.node {
                Item::Struct(s) => self.check_struct(s),
                Item::Message(m) => self.check_message(m),
                Item::Enum(e) => self.check_enum(e),
                Item::Union(u) => self.check_union(u),
            }
        }

        self.errors
    }

    fn check_duplicate_definitions(&mut self) {
        // Build a map of all defined types
        let mut definitions: HashMap<&str, Vec<&Spanned<Item>>> = HashMap::new();
        for item in &self.schema.items {
            let name = item.name();
            definitions.entry(name).or_default().push(item);
        }

        // Check for duplicate definitions
        for (name, items) in &definitions {
            if items.len() > 1 {
                let mut err = CheckError::new(format!("duplicate definition of `{}`", name));
                for (i, item) in items.iter().enumerate() {
                    let label = if i == 0 { "first defined here" } else { "redefined here" };
                    let color = if i == 0 { Color::Blue } else { Color::Red };
                    err = err.label(item_name_span(item), label, color);
                }
                self.errors.push(err);
            }
        }
    }

    fn check_struct(&mut self, s: &Struct) {
        // Check for duplicate field names
        let mut field_names: HashMap<&str, &Spanned<StructField>> = HashMap::new();
        for field in &s.fields {
            if let Some(prev) = field_names.get(field.name.as_str()) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate field `{}` in struct `{}`",
                        field.name.node, s.name.node
                    ))
                    .label(prev.name.span.clone(), "first defined here", Color::Blue)
                    .label(field.name.span.clone(), "redefined here", Color::Red),
                );
            } else {
                field_names.insert(&field.name.node, field);
            }

            // Check type references
            self.check_type(&field.ty);
        }
    }

    fn check_message(&mut self, m: &Message) {
        // Check for duplicate field names
        let mut field_names: HashMap<&str, &Spanned<MessageField>> = HashMap::new();
        for field in &m.fields {
            if let Some(prev) = field_names.get(field.name.as_str()) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate field `{}` in message `{}`",
                        field.name.node, m.name.node
                    ))
                    .label(prev.name.span.clone(), "first defined here", Color::Blue)
                    .label(field.name.span.clone(), "redefined here", Color::Red),
                );
            } else {
                field_names.insert(&field.name.node, field);
            }
        }

        // Check for duplicate field indices and zero index
        let mut field_indices: HashMap<u32, &Spanned<MessageField>> = HashMap::new();
        for field in &m.fields {
            // Check for zero index (reserved for termination)
            if field.index.node == 0 {
                self.errors.push(
                    CheckError::new(format!("index 0 is reserved in message `{}`", m.name.node)).label(
                        field.index.span.clone(),
                        "indices must be >= 1",
                        Color::Red,
                    ),
                );
            }

            if let Some(prev) = field_indices.get(&field.index.node) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate index {} in message `{}`",
                        field.index.node, m.name.node
                    ))
                    .label(prev.index.span.clone(), "first used here", Color::Blue)
                    .label(field.index.span.clone(), "reused here", Color::Red),
                );
            } else {
                field_indices.insert(field.index.node, field);
            }

            // Check type references
            self.check_type(&field.ty);
        }
    }

    fn check_enum(&mut self, e: &Enum) {
        // Check for duplicate variant names
        let mut variant_names: HashMap<&str, &Spanned<EnumVariant>> = HashMap::new();
        for variant in &e.variants {
            if let Some(prev) = variant_names.get(variant.name.as_str()) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate variant `{}` in enum `{}`",
                        variant.name.node, e.name.node
                    ))
                    .label(prev.name.span.clone(), "first defined here", Color::Blue)
                    .label(variant.name.span.clone(), "redefined here", Color::Red),
                );
            } else {
                variant_names.insert(&variant.name.node, variant);
            }
        }

        // Check for duplicate variant indices
        let mut variant_indices: HashMap<u32, &Spanned<EnumVariant>> = HashMap::new();
        for variant in &e.variants {
            if let Some(prev) = variant_indices.get(&variant.index.node) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate index {} in enum `{}`",
                        variant.index.node, e.name.node
                    ))
                    .label(prev.index.span.clone(), "first used here", Color::Blue)
                    .label(variant.index.span.clone(), "reused here", Color::Red),
                );
            } else {
                variant_indices.insert(variant.index.node, variant);
            }
        }
    }

    fn check_union(&mut self, u: &Union) {
        // Check for duplicate variant names
        let mut variant_names: HashMap<&str, &Spanned<UnionVariant>> = HashMap::new();
        for variant in &u.variants {
            if let Some(prev) = variant_names.get(variant.name.as_str()) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate variant `{}` in union `{}`",
                        variant.name.node, u.name.node
                    ))
                    .label(prev.name.span.clone(), "first defined here", Color::Blue)
                    .label(variant.name.span.clone(), "redefined here", Color::Red),
                );
            } else {
                variant_names.insert(&variant.name.node, variant);
            }
        }

        // Check for duplicate variant indices and zero index
        let mut variant_indices: HashMap<u32, &Spanned<UnionVariant>> = HashMap::new();
        for variant in &u.variants {
            // Check for zero index (reserved for termination in wire format)
            if variant.index.node == 0 {
                self.errors.push(
                    CheckError::new(format!("index 0 is reserved in union `{}`", u.name.node)).label(
                        variant.index.span.clone(),
                        "indices must be >= 1",
                        Color::Red,
                    ),
                );
            }

            if let Some(prev) = variant_indices.get(&variant.index.node) {
                self.errors.push(
                    CheckError::new(format!(
                        "duplicate index {} in union `{}`",
                        variant.index.node, u.name.node
                    ))
                    .label(prev.index.span.clone(), "first used here", Color::Blue)
                    .label(variant.index.span.clone(), "reused here", Color::Red),
                );
            } else {
                variant_indices.insert(variant.index.node, variant);
            }

            // Check type references
            if let Some(ty) = &variant.ty {
                self.check_type(ty);
            }
        }
    }

    fn check_type(&mut self, ty: &Spanned<Type>) {
        match &ty.node {
            Type::Named(name) => {
                if self.schema.item(name.as_str()).is_none() {
                    self.errors
                        .push(CheckError::new(format!("undefined type `{}`", name)).label(
                            ty.span.clone(),
                            "not found",
                            Color::Red,
                        ));
                }
            }
            Type::Array(inner) => self.check_type(inner),
            Type::Map(key, value) => {
                self.check_type(key);
                self.check_type(value);

                // Validate that the key type is valid for maps
                self.check_map_key_type(key);
            }
            _ => {}
        }
    }

    /// Checks if a type can be used as a map key.
    ///
    /// Map keys must be comparable and hashable across all target languages (Go, JS, Rust).
    /// Valid key types are the lowest common denominator:
    /// - Primitive integers: u8, u16, u32, u64, i8, i16, i32, i64
    /// - bool
    /// - string
    /// - Enums (simple tag values)
    /// - Structs without floats, arrays, maps, or optional fields
    ///
    /// Invalid key types:
    /// - f32, f64 (not Eq+Hash in Rust, not comparable in Go)
    /// - Arrays, Maps (not comparable in Go)
    /// - Messages, Unions (not comparable)
    /// - Structs with floats, optional fields, or complex nested types
    fn check_map_key_type(&mut self, key_ty: &Spanned<Type>) {
        if !self.is_valid_map_key(&key_ty.node) {
            let reason = self.get_invalid_key_reason(&key_ty.node);
            self.errors
                .push(CheckError::new(format!("invalid map key type: {}", reason)).label(
                    key_ty.span.clone(),
                    "cannot be used as map key",
                    Color::Red,
                ));
        }
    }

    fn is_valid_map_key(&self, ty: &Type) -> bool {
        match ty {
            // Valid primitive types
            Type::Bool
            | Type::U8
            | Type::U16
            | Type::U32
            | Type::U64
            | Type::I8
            | Type::I16
            | Type::I32
            | Type::I64
            | Type::String => true,

            // Floats are not valid (not Eq+Hash in Rust, not comparable in Go)
            Type::F32 | Type::F64 => false,

            // Arrays and maps are not valid (not comparable in Go)
            Type::Array(_) | Type::Map(_, _) => false,

            // Check named types
            Type::Named(name) => {
                if let Some(item) = self.schema.item(name.as_str()) {
                    match &item.node {
                        // Enums are valid (simple comparable values)
                        Item::Enum(_) => true,

                        // Structs are valid only if all fields are valid map keys
                        // and there are no optional fields
                        Item::Struct(s) => self.is_struct_valid_map_key(s),

                        // Messages and unions are not valid (not comparable)
                        Item::Message(_) | Item::Union(_) => false,
                    }
                } else {
                    // Undefined type - will be caught by check_type
                    true
                }
            }
        }
    }

    fn is_struct_valid_map_key(&self, s: &Struct) -> bool {
        // Structs with optional fields are not valid (presence bits make them not comparable)
        if s.fields.iter().any(|f| f.optional) {
            return false;
        }

        // All fields must be valid map key types
        s.fields.iter().all(|f| self.is_valid_map_key(&f.ty.node))
    }

    fn get_invalid_key_reason(&self, ty: &Type) -> String {
        match ty {
            Type::F32 | Type::F64 => {
                "floating-point types cannot be used as map keys (not Eq+Hash in Rust, not comparable in Go)"
                    .to_string()
            }
            Type::Array(_) => "arrays cannot be used as map keys (not comparable in Go)".to_string(),
            Type::Map(_, _) => "maps cannot be used as map keys (not comparable in Go)".to_string(),
            Type::Named(name) => {
                if let Some(item) = self.schema.item(name.as_str()) {
                    match &item.node {
                        Item::Message(_) => {
                            format!("messages cannot be used as map keys (not comparable)")
                        }
                        Item::Union(_) => {
                            format!("unions cannot be used as map keys (not comparable)")
                        }
                        Item::Struct(s) => {
                            if s.fields.iter().any(|f| f.optional) {
                                format!("struct '{}' has optional fields (not comparable)", name)
                            } else {
                                // Find the problematic field
                                for field in &s.fields {
                                    if !self.is_valid_map_key(&field.ty.node) {
                                        return format!(
                                            "struct '{}' has field '{}' of invalid type for map keys",
                                            name, field.name.node
                                        );
                                    }
                                }
                                format!("struct '{}' cannot be used as map key", name)
                            }
                        }
                        _ => format!("type '{}' cannot be used as map key", name),
                    }
                } else {
                    format!("type '{}' cannot be used as map key", name)
                }
            }
            _ => "this type cannot be used as a map key".to_string(),
        }
    }
}

fn item_name_span(item: &Spanned<Item>) -> Span {
    match &item.node {
        Item::Struct(s) => s.name.span.clone(),
        Item::Message(m) => m.name.span.clone(),
        Item::Enum(e) => e.name.span.clone(),
        Item::Union(u) => u.name.span.clone(),
    }
}

pub fn check(schema: &Schema) -> Vec<CheckError> {
    Checker::new(schema).check()
}

pub fn print_errors(filename: &str, src: &str, errors: Vec<CheckError>) {
    for err in errors {
        let start = err.labels.first().map(|(s, _, _)| s.start).unwrap_or(0);
        let mut report = Report::build(ReportKind::Error, filename, start).with_message(&err.message);

        for (span, message, color) in err.labels {
            report = report.with_label(Label::new((filename, span)).with_message(message).with_color(color));
        }

        report.finish().print((filename, Source::from(src))).unwrap();
    }
}
