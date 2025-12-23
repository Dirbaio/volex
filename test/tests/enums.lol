// Comprehensive enum tests

// Simple enum with sequential indices
enum Status {
    Idle = 0
    Active = 1
    Paused = 2
    Stopped = 3
}

// Enum with explicit non-sequential indices
enum Priority {
    Low = 0
    Medium = 5
    High = 10
    Critical = 100
}

// Single variant enum
enum Unit {
    Value = 0
}

// Enum with many variants
enum Color {
    Red = 0
    Orange = 1
    Yellow = 2
    Green = 3
    Blue = 4
    Indigo = 5
    Violet = 6
    Black = 7
    White = 8
    Gray = 9
}

// Enum used in struct
struct Task {
    id: u32
    status: Status
    priority: Priority
}

// Enum used in message
message Job {
    id: u32 = 1
    status: Status = 2
    priority: Priority = 3
}

// Array of enums
struct StatusList {
    statuses: [Status]
}

// Optional enum
struct OptionalStatus {
    status?: Status
}
