// Comprehensive struct tests

// Empty struct
struct Empty {
}

// Single field structs
struct SingleU8 {
    value: u8
}

struct SingleString {
    value: string
}

// Multi-field struct with all fixed-size fields
struct Point2D {
    x: f32
    y: f32
}

struct Point3D {
    x: f32
    y: f32
    z: f32
}

// Mixed fixed and variable size fields
struct Person {
    age: u8
    name: string
    height: f32
}

// Nested structs
struct Line {
    start: Point2D
    end: Point2D
}

struct Transform {
    position: Point3D
    scale: Point3D
}

// Struct with single optional field
struct OptionalU8 {
    value?: u8
}

struct OptionalString {
    value?: string
}

// Struct with mix of required and optional fields
struct User {
    id: u32
    username: string
    email?: string
    age?: u8
}

// Struct with multiple optional fields
struct Config {
    timeout?: u32
    retry_count?: u16
    endpoint?: string
    enabled?: bool
}

// Struct with all optional fields
struct AllOptional {
    a?: u8
    b?: u16
    c?: string
}

// Struct with many optional fields (tests multiple presence bytes)
struct ManyOptionals {
    f1?: u8
    f2?: u8
    f3?: u8
    f4?: u8
    f5?: u8
    f6?: u8
    f7?: u8
    f8?: u8
    f9?: u8
    f10?: u8
}

// Complex nested struct with optionals
struct Profile {
    user_id: u64
    display_name: string
    bio?: string
    location?: string
    verified: bool
}

// Struct with optional nested struct
struct Container {
    id: u32
    point?: Point2D
    name: string
}
