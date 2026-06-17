# TOON Formal Grammar — ASL v0.1.0

Grammar notation: **EBNF** — `[ ]` optional, `{ }` zero-or-more, `( )` grouping, `|` alternation, `"text"` terminal.

---

## Top-Level Document

```ebnf
toon_document ::=
    metadata_block
    hardware_block
    data_block
    setup_block
    loop_block
    functions_block?
```

---

## Metadata Block

```ebnf
metadata_block ::=
    "# METADATA:"
    "project_name:" string_literal
    "version:"      integer
    "editor:"       identifier
    "author:"       identifier
    "ASLversion:"   version_literal

version_literal ::= digit+ "." digit+ "." digit+
```

---

## Hardware Block

```ebnf
hardware_block ::=
    "## 1. HARDWARE INTERFACE:"
    "hardware[" uint "|]{pin|type|io_mode|label|target_role}:"
    { hardware_entry }

hardware_entry ::=
    uint "|" hw_type "|" io_mode "|" label "|" target_role

hw_type     ::= "BOOL" | "analog" | "digital"
io_mode     ::= "input" | "output" | "bi" | "DO" | "DI"
label       ::= quoted_string | percent_label
target_role ::=
    "digital-out" | "digital-in" | "pwm-out" |
    "adc:t"       | "adc:raw"    |
    "i2c-sda"     | "i2c-scl"   |
    "spi-mosi"    | "spi-miso"  | "spi-sck" | "spi-cs" |
    "uart-tx"     | "uart-rx"   |
    "int-rising"  | "int-falling" | "int-change"

percent_label ::= "%" quoted_string
quoted_string ::= '"' { any_char } '"'
```

---

## Data Block

```ebnf
data_block ::=
    "## 2. SYSTEM DATA:"
    "data[" uint "|]{name|type|scope|init_value|unit}:"
    { data_entry }

data_entry ::=
    identifier "|" data_type "|" scope "|" init_value "|" unit

data_type ::=
    "BOOL"  | "INT"   | "DINT"  | "UINT"  |
    "REAL"  | "LREAL" | "TIME"  | "STRING" |
    "BYTE"  | "WORD"  | "DWORD"

scope ::=
    "CONST" | "VAR" | "RETAIN" | "PERSISTENT" |
    "TEMP"  | "EXTERN"

init_value ::=
    bool_literal    |
    integer_literal |
    real_literal    |
    time_literal    |
    string_literal  |
    hex_literal     |
    "FALSE" | "TRUE"

unit ::= string_literal | "-" | "ms" | "us" | "s" | "C" | "%" | "mm"

time_literal ::= "T#" digit+ time_unit
time_unit    ::= "ms" | "us" | "s" | "min" | "h"
hex_literal  ::= "0x" hex_digit+ | "16#" hex_digit+
hex_digit    ::= digit | "A"-"F" | "a"-"f"
```

---

## Setup Block

```ebnf
setup_block ::=
    "## 3. INITIALIZATION SYSTEM:"
    "setup[" uint "]:"
    { "  - " statement }
```

---

## Loop Block

```ebnf
loop_block ::=
    "## 4. BEHAVIORAL LOGIC (LOOP):"
    "loop[" uint "]:"
    { "  - " statement }
```

---

## Functions Block

```ebnf
functions_block ::=
    "## 5. PROCEDURAL FUNCTIONS:"
    "functions:"
    { function_def }

function_def ::=
    indent function_name "(" param_list? ")" "[" uint "]:"
    { indent indent "- " statement }

function_name ::= identifier
param_list    ::= param { "," param }
param         ::= identifier
```

---

## Statements

```ebnf
statement ::=
    assignment_stmt     |
    call_stmt           |
    control_flow_stmt   |
    return_stmt         |
    wait_stmt           |
    cfg_stmt            |
    sys_stmt            |
    vendor_ext_stmt

assignment_stmt ::=
    identifier "=" expression

call_stmt ::=
    stdlib_call | user_call

return_stmt ::=
    "RETURN" expression |
    "RETURN NOT" identifier

wait_stmt ::=
    "WAIT_TIME(" time_arg ")" |
    "WAIT_TIME_US(" time_arg ")"

time_arg ::= time_literal | identifier

cfg_stmt ::=
    "CFG_DIGITAL_OUTPUT(" uint_arg ")"                              |
    "CFG_DIGITAL_INPUT(" uint_arg ")"                               |
    "CFG_DIGITAL_INPUT_PULLUP(" uint_arg ")"                        |
    "CFG_DIGITAL_INPUT_PULLDOWN(" uint_arg ")"                      |
    "CFG_PWM_OUTPUT(" uint_arg "," uint_arg ")"                     |
    "CFG_ADC_INPUT(" uint_arg ")"                                   |
    "CFG_SERIAL(" dint_arg ")"                                      |
    "CFG_I2C(" uint_arg "," uint_arg "," uint_arg "," dint_arg ")"  |
    "CFG_SPI(" uint_arg "," uint_arg "," int_arg "," uint_arg "," uint_arg ")" |
    "CFG_SENSOR_I2C()"                                              |
    "CFG_TFT(" uint_arg "," int_arg "," int_arg ")"                 |
    "CFG_LCD(" byte_arg "," uint_arg "," uint_arg ")"               |
    "CFG_ATTACH_INTERRUPT(" uint_arg "," isr_ref "," edge_mode ")" |
    "CFG_DETACH_INTERRUPT(" uint_arg ")"

sys_stmt ::=
    "SYS_LOAD_MEMORY_SECTOR(RETAIN_SECTOR)"      |
    "SYS_LOAD_MEMORY_SECTOR(PERSISTENT_SECTOR)"  |
    "SYS_WRITE_RETAIN(" var_ref "," expression ")" |
    "SYS_READ_RETAIN(" var_ref ")"

vendor_ext_stmt ::=
    "VENDOR_EXTENSION(" quoted_string ")"

edge_mode ::= "RISING" | "FALLING" | "CHANGE"
isr_ref   ::= identifier   (* must resolve to a functions: entry *)
var_ref   ::= identifier   (* must resolve to a data: entry *)
uint_arg  ::= uint | identifier
int_arg   ::= integer | identifier
dint_arg  ::= integer | identifier
byte_arg  ::= uint | identifier
```

---

## Control Flow Statements

```ebnf
control_flow_stmt ::=
    if_stmt       |
    elsif_stmt    |
    else_stmt     |
    while_stmt    |
    for_stmt      |
    break_stmt    |
    continue_stmt

if_stmt       ::= "IF" condition ":" | "IF NOT" condition ":"
elseif_stmt   ::= "ELSIF" condition ":"
else_stmt     ::= "ELSE:"
while_stmt    ::= "WHILE" condition ":" | "WHILE TRUE:"
for_stmt      ::= "FOR _i" depth " := 0 TO " expression ":"
                | "repeat(" expression "):"    (* alias *)
break_stmt    ::= "EXIT" | "break"
continue_stmt ::= "CONTINUE" | "continue"

condition ::= expression
depth     ::= digit   (* 0,1,2... for nested loops — generator-internal *)
```

---

## Expressions

```ebnf
expression ::=
    unary_expr                          |
    expression binary_op expression     |
    function_call                       |
    cast_expr                           |
    concat_expr                         |
    "(" expression ")"

unary_expr ::=
    "NOT" expression          |
    "NOT_BIT(" expression ")" |
    "-" expression            |
    primary

primary ::=
    identifier    |
    literal       |
    function_call

binary_op ::=
    arithmetic_op | comparison_op | bool_op | bitwise_op

arithmetic_op ::= "+" | "-" | "*" | "/" | "MOD"
comparison_op ::= ">" | "<" | ">=" | "<=" | "=" | "<>"
bool_op       ::= "AND" | "OR" | "XOR"
bitwise_op    ::= "AND_BIT" | "OR_BIT" | "XOR_BIT" | "SHL" | "SHR" | "ROL" | "ROR"

cast_expr ::=
    "REAL_TO_INT(" expression ")"    |
    "INT_TO_REAL(" expression ")"    |
    "BOOL_TO_INT(" expression ")"    |
    "INT_TO_BOOL(" expression ")"    |
    "WORD_TO_INT(" expression ")"    |
    "INT_TO_WORD(" expression ")"    |
    "INT_TO_DINT(" expression ")"    |
    "REAL_TO_LREAL(" expression ")"  |
    "TRUNC(" expression ")"          |
    "int(" expression ")"            (* alias → TRUNC *)

concat_expr ::=
    string_part { "+" string_part }

string_part ::=
    string_literal                              |
    "TO_STRING(" expression ")"                 |
    "TO_STRING_FMT(" expression "," uint_arg ")"
```

---

## Stdlib Function Calls

```ebnf
stdlib_call ::=
    (* GPIO *)
    "SET_BOOL(" uint_arg "," bool_expr ")"       |
    "GET_BOOL(" uint_arg ")"                     |

    (* PWM *)
    "SET_PWM_FREQ(" uint_arg "," uint_arg ")"    |
    "SET_PWM_DUTY(" uint_arg "," uint_arg ")"    |
    "GET_PWM_DUTY(" uint_arg ")"                 |

    (* ADC *)
    "READ_ADC(" uint_arg ")"                     |

    (* Time *)
    "GET_SYS_TIME()"                             |
    "GET_SYS_TIME_US()"                          |
    "TIME_DIFF(" dword_arg "," dword_arg ")"     |

    (* Math *)
    "SQRT(" expression ")"                       |
    "LN(" expression ")"                         |
    "LOG(" expression ")"                        |
    "ABS(" expression ")"                        |
    "EXPT(" expression "," expression ")"        |
    "FLOOR(" expression ")"                      |
    "CEIL(" expression ")"                       |
    "MAX(" expression "," expression ")"         |
    "MIN(" expression "," expression ")"         |
    "MOD(" expression "," expression ")"         |
    "SEL(" bool_expr "," expression "," expression ")" |
    "MAP_RANGE(" expression "," expression "," expression "," expression "," expression ")" |

    (* String *)
    "CONCAT(" string_part { "," string_part } ")"    |
    "TO_STRING(" expression ")"                      |
    "TO_STRING_FMT(" expression "," uint_arg ")"     |

    (* Sensor *)
    "READ_TEMPERATURE()"    |
    "READ_HUMIDITY()"       |
    "READ_PRESSURE()"       |

    (* Serial *)
    "SERIAL_PRINT(" expression ")"       |
    "SERIAL_PRINTLN(" expression ")"     |
    "SERIAL_READLN()"                    |
    "SERIAL_READ()"                      |
    "SERIAL_AVAILABLE()"                 |
    "SERIAL_FLUSH()"                     |

    (* Display — TFT *)
    "TFT_FILL(" word_arg ")"             |
    "TFT_SET_TEXT_COLOR(" word_arg "," word_arg ")"  |
    "TFT_SET_TEXT_SIZE(" uint_arg ")"    |
    "TFT_PRINT(" uint_arg "," uint_arg "," expression ")"  |
    "TFT_DRAW_ROUND_RECT(" uint_arg "," uint_arg "," uint_arg "," uint_arg "," uint_arg "," word_arg ")"  |

    (* Display — LCD *)
    "LCD_CLEAR()"                                            |
    "LCD_PRINT(" uint_arg "," uint_arg "," expression ")"

dword_arg ::= identifier | integer
word_arg  ::= identifier | hex_literal | integer
bool_expr ::= expression   (* type-checked: must resolve to BOOL *)
```

---

## User Function Calls

```ebnf
user_call ::=
    identifier "(" arg_list? ")"

arg_list ::= expression { "," expression }
```

---

## Literals

```ebnf
literal ::=
    bool_literal    |
    integer_literal |
    real_literal    |
    time_literal    |
    string_literal  |
    hex_literal

bool_literal    ::= "TRUE" | "FALSE" | "true" | "false"
integer_literal ::= [ "-" ] digit+
real_literal    ::= [ "-" ] digit+ "." digit+
string_literal  ::= '"' { any_char_except_quote } '"'
time_literal    ::= "T#" digit+ time_unit
hex_literal     ::= "0x" hex_digit+ | "16#" hex_digit+
```

---

## Lexical Primitives

```ebnf
identifier  ::= alpha { alpha | digit | "_" }
alpha       ::= "A"-"Z" | "a"-"z" | "_"
digit       ::= "0"-"9"
uint        ::= digit+
integer     ::= [ "-" ] digit+
indent      ::= "  "          (* 2 spaces per level *)
comment     ::= "#" { any_char } newline
newline     ::= "\n" | "\r\n"
any_char    ::= (* any Unicode codepoint except newline *)
```

---

## Alias Normalization Table (Parser Pass 0)

Before any grammar rule is applied, the lexer normalizes all aliases to their IEC canonical names:

```
digitalWrite        → SET_BOOL
digitalRead         → GET_BOOL
pwmFreq             → SET_PWM_FREQ
pwmWrite            → SET_PWM_DUTY
pwmRead             → GET_PWM_DUTY
readADC             → READ_ADC
currentTime         → GET_SYS_TIME
currentTimeUS       → GET_SYS_TIME_US
delay               → WAIT_TIME
sqrt                → SQRT
log                 → LN
log10               → LOG
abs                 → ABS
pow                 → EXPT
floor               → FLOOR
ceil                → CEIL
max                 → MAX
min                 → MIN
int                 → TRUNC
string(x)           → TO_STRING
string(x,d)         → TO_STRING_FMT
map_range           → MAP_RANGE
attachInterrupt     → CFG_ATTACH_INTERRUPT
detachInterrupt     → CFG_DETACH_INTERRUPT
initI2C             → CFG_I2C
initSPI             → CFG_SPI
serialBegin         → CFG_SERIAL
initTFT             → CFG_TFT
initLCD             → CFG_LCD
initSensorI2C       → CFG_SENSOR_I2C
readTemperature     → READ_TEMPERATURE
readHumidity        → READ_HUMIDITY
readPressure        → READ_PRESSURE
tftFillScreen       → TFT_FILL
tftSetTextColor     → TFT_SET_TEXT_COLOR
tftSetTextSize      → TFT_SET_TEXT_SIZE
tftPrintText        → TFT_PRINT
tftDrawRoundRect    → TFT_DRAW_ROUND_RECT
lcdClear            → LCD_CLEAR
lcdPrint            → LCD_PRINT
if                  → IF
else                → ELSE
elif                → ELSIF
repeat              → FOR
break               → EXIT
continue            → CONTINUE
16#                 → 0x    (hex prefix)
```

---

## Parse Error Catalogue

| Code | Condition |
|---|---|
| `PARSE_ERROR_001` | Undeclared identifier: `<name>` not in `data[]` or `hardware[]` |
| `PARSE_ERROR_002` | Type mismatch: expected `<T1>`, got `<T2>` at `<expr>` |
| `PARSE_ERROR_003` | Raw time subtraction: use `TIME_DIFF(a, b)` instead of `a - b` |
| `PARSE_ERROR_004` | Pin integer emitted directly: resolve `<n>` via `hardware[]` label |
| `PARSE_ERROR_005` | Unresolved ISR reference: `<name>` not in `functions:` |
| `PARSE_ERROR_006` | Unresolved function call: `<name>` not in stdlib or `functions:` |
| `PARSE_ERROR_007` | `VENDOR_EXTENSION` required: `<fn>` could not be resolved |
| `PARSE_ERROR_008` | `data[]` count mismatch: header declares `[N]` but `<M>` entries found |
| `PARSE_ERROR_009` | `hardware[]` count mismatch: header declares `[N]` but `<M>` entries found |
| `PARSE_ERROR_010` | `setup[]` step count mismatch: header declares `[N]` but `<M>` steps found |
| `PARSE_ERROR_011` | `loop[]` step count mismatch: header declares `[N]` but `<M>` steps found |
| `PARSE_ERROR_012` | `functions:` step count mismatch: `<fn>[N]` declares N but `<M>` steps found |
| `PARSE_ERROR_013` | Nested `repeat`/`FOR` depth exceeds maximum (4 levels) |
| `PARSE_ERROR_014` | `RETAIN`/`PERSISTENT` variable `<name>` written without `SYS_WRITE_RETAIN` |
| `PARSE_ERROR_015` | `isr_*` function contains heap-allocating call: `<call>` |

---

## Symbol Table (Built at Parse Time)

```
symbol_table ::= {
  hardware_map:     { pin_number → { label, type, io_mode, target_role } }
  data_map:         { name → { data_type, scope, init_value, unit } }
  function_map:     { name → { params[], step_count, body[] } }
  pwm_channel_map:  { pin_number → channel_index }   (* generator-internal *)
  adc_object_map:   { pin_number → adc_index }        (* generator-internal *)
  uart_object_map:  { pin_number → uart_index }       (* generator-internal *)
  isr_written_vars: [ var_name ]                      (* for volatile injection *)
  time_typed_vars:  [ var_name ]                      (* for TIME_DIFF enforcement *)
}
```
