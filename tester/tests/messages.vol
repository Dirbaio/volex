// Comprehensive message tests

// Empty message
message EmptyMessage {
}

// Single field messages with different wire types
message SingleU8 {
    value: u8 = 1  // FIXED8
}

message SingleU32 {
    value: u32 = 1  // VARINT
}

message SingleF32 {
    value: f32 = 1  // FIXED32
}

message SingleF64 {
    value: f64 = 1  // FIXED64
}

message SingleString {
    value: string = 1  // BYTES (no inner length prefix)
}

message SingleArray {
    values: [u32] = 1  // BYTES (with count for variable-size elements)
}

message SingleArrayFixed {
    values: [u8] = 1  // BYTES (no count for fixed-size elements)
}

// Multiple fields with different wire types
message AllWireTypes {
    fixed8: u8 = 1       // FIXED8
    varint: u32 = 2      // VARINT
    fixed32: f32 = 3     // FIXED32
    fixed64: f64 = 4     // FIXED64
    bytes: string = 5    // BYTES
}

// Messages with optional fields
message OptionalFields {
    required_id: u32 = 1
    optional_name?: string = 2
    optional_age?: u8 = 3
}

// Nested messages
struct Point {
    x: f32
    y: f32
}

message NestedStruct {
    id: u32 = 1
    point: Point = 2  // BYTES (struct is variable-size due to message field)
}

message NestedMessage {
    id: u32 = 1
    inner: SingleU32 = 2  // MESSAGE
}

// Test default values - fields not present should use defaults
message WithDefaults {
    number: u32 = 1
    text: string = 2
    flag: bool = 3
    list: [u32] = 4
}

// Test field ordering - fields must be encoded in index order
message FieldOrdering {
    field3: u32 = 3
    field1: u32 = 1
    field2: u32 = 2
}

// Test sparse field indices
message SparseIndices {
    field1: u32 = 1
    field10: u32 = 10
    field100: u32 = 100
}

// Arrays in messages - fixed vs variable size elements
message ArraysInMessage {
    fixed_array: [u8] = 1      // BYTES, no count (fixed-size elements)
    variable_array: [string] = 2  // BYTES, with count (variable-size elements)
}

// String in message (no inner length prefix)
message StringInMessage {
    id: u32 = 1
    name: string = 2  // String data is just raw UTF-8, length from BYTES wire type
}

// Complex nested structure
message UserProfile {
    id: u64 = 1
    username: string = 2
    email?: string = 3
    verified: bool = 4
    metadata?: NestedMessage = 5
}

// Message with all optional fields
message AllOptionalMessage {
    opt1?: u32 = 1
    opt2?: string = 2
    opt3?: bool = 3
}

// Testing backwards/forwards compatibility
message VersionedMessage {
    version: u32 = 1
    data: string = 2
}

message ExtendedVersionedMessage {
    version: u32 = 1
    data: string = 2
    extra_field?: u32 = 3
    new_field?: string = 4
}

// Struct with only fixed-size fields (fixed-size struct) - with floats
struct FixedSizeStructWithFloat {
    a: u8
    b: f32
    c: u8
}

// Struct with only fixed-size fields that can be used as map key (no floats)
struct FixedSizeStruct {
    a: u8
    b: u16
    c: u8
}

// Struct with variable-size field (variable-size struct)
struct VariableSizeStruct {
    id: u32
    name: string
}

// Arrays of structs - fixed-size elements (no count prefix)
message ArrayOfFixedStruct {
    items: [FixedSizeStruct] = 1  // BYTES, no count (struct is fixed-size)
}

message ArrayOfFixedStructWithFloat {
    items: [FixedSizeStructWithFloat] = 1  // BYTES, no count (struct is fixed-size)
}

// Arrays of structs - variable-size elements (with count prefix)
message ArrayOfVariableStruct {
    items: [VariableSizeStruct] = 1  // BYTES, with count (struct has string)
}

// Maps with u8 key
message MapU8ToU8 {
    map: {u8: u8} = 1  // BYTES, no count (both key and value fixed-size)
}

message MapU8ToFixedStruct {
    map: {u8: FixedSizeStruct} = 1  // BYTES, no count (both fixed-size)
}

message MapU8ToString {
    map: {u8: string} = 1  // BYTES, with count (value is variable-size)
}

// Maps with fixed-size struct key
message MapFixedStructToU8 {
    map: {FixedSizeStruct: u8} = 1  // BYTES, no count (both fixed-size)
}

message MapFixedStructToFixedStruct {
    map: {FixedSizeStruct: FixedSizeStruct} = 1  // BYTES, no count (both fixed-size)
}

message MapFixedStructToString {
    map: {FixedSizeStruct: string} = 1  // BYTES, with count (value is variable-size)
}

// Maps with string key
message MapStringToU8 {
    map: {string: u8} = 1  // BYTES, with count (key is variable-size)
}

message MapStringToFixedStruct {
    map: {string: FixedSizeStruct} = 1  // BYTES, with count (key is variable-size)
}

message MapStringToString {
    map: {string: string} = 1  // BYTES, with count (both variable-size)
}
