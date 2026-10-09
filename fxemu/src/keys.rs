//! fx-9860GII keyboard: key codes (row << 4 | (7 - column)) as used by gint,
//! and their position in the KEYSC scan matrix.

pub struct Key {
    pub code: u8,
    pub name: &'static str,
    pub label: &'static str,
}

/// Keypad layout, row by row, as on the calculator.
pub const LAYOUT: &[&[Key]] = &[
    &[
        Key { code: 0x91, name: "F1", label: "F1" },
        Key { code: 0x92, name: "F2", label: "F2" },
        Key { code: 0x93, name: "F3", label: "F3" },
        Key { code: 0x94, name: "F4", label: "F4" },
        Key { code: 0x95, name: "F5", label: "F5" },
        Key { code: 0x96, name: "F6", label: "F6" },
    ],
    &[
        Key { code: 0x81, name: "SHIFT", label: "SHIFT" },
        Key { code: 0x82, name: "OPTN", label: "OPTN" },
        Key { code: 0x83, name: "VARS", label: "VARS" },
        Key { code: 0x84, name: "MENU", label: "MENU" },
        Key { code: 0x85, name: "LEFT", label: "◀" },
        Key { code: 0x86, name: "UP", label: "▲" },
    ],
    &[
        Key { code: 0x71, name: "ALPHA", label: "ALPHA" },
        Key { code: 0x72, name: "SQUARE", label: "x²" },
        Key { code: 0x73, name: "POWER", label: "^" },
        Key { code: 0x74, name: "EXIT", label: "EXIT" },
        Key { code: 0x75, name: "DOWN", label: "▼" },
        Key { code: 0x76, name: "RIGHT", label: "▶" },
    ],
    &[
        Key { code: 0x61, name: "XOT", label: "X,θ,T" },
        Key { code: 0x62, name: "LOG", label: "log" },
        Key { code: 0x63, name: "LN", label: "ln" },
        Key { code: 0x64, name: "SIN", label: "sin" },
        Key { code: 0x65, name: "COS", label: "cos" },
        Key { code: 0x66, name: "TAN", label: "tan" },
    ],
    &[
        Key { code: 0x51, name: "FRAC", label: "a b/c" },
        Key { code: 0x52, name: "FD", label: "F↔D" },
        Key { code: 0x53, name: "LEFTP", label: "(" },
        Key { code: 0x54, name: "RIGHTP", label: ")" },
        Key { code: 0x55, name: "COMMA", label: "," },
        Key { code: 0x56, name: "ARROW", label: "→" },
    ],
    &[
        Key { code: 0x41, name: "7", label: "7" },
        Key { code: 0x42, name: "8", label: "8" },
        Key { code: 0x43, name: "9", label: "9" },
        Key { code: 0x44, name: "DEL", label: "DEL" },
        Key { code: 0x07, name: "ACON", label: "AC/ON" },
    ],
    &[
        Key { code: 0x31, name: "4", label: "4" },
        Key { code: 0x32, name: "5", label: "5" },
        Key { code: 0x33, name: "6", label: "6" },
        Key { code: 0x34, name: "MUL", label: "×" },
        Key { code: 0x35, name: "DIV", label: "÷" },
    ],
    &[
        Key { code: 0x21, name: "1", label: "1" },
        Key { code: 0x22, name: "2", label: "2" },
        Key { code: 0x23, name: "3", label: "3" },
        Key { code: 0x24, name: "ADD", label: "+" },
        Key { code: 0x25, name: "SUB", label: "−" },
    ],
    &[
        Key { code: 0x11, name: "0", label: "0" },
        Key { code: 0x12, name: "DOT", label: "." },
        Key { code: 0x13, name: "EXP", label: "EXP" },
        Key { code: 0x14, name: "NEG", label: "(−)" },
        Key { code: 0x15, name: "EXE", label: "EXE" },
    ],
];

pub fn by_name(name: &str) -> Option<u8> {
    let up = name.to_ascii_uppercase();
    LAYOUT.iter().flat_map(|r| r.iter()).find(|k| k.name == up).map(|k| k.code)
}

/// Set or clear one key in a 12-byte KEYSC scan matrix.
pub fn set_matrix(m: &mut [u8; 12], code: u8, down: bool) {
    let row = (code >> 4) as usize;
    let col = 7 - (code & 0xF) as u32;
    if row >= 12 || col > 7 {
        return;
    }
    if down {
        m[row] |= 1 << col;
    } else {
        m[row] &= !(1 << col);
    }
}
