# TOON Standard Library — ASL v0.1.0

> IEC 61131-3 Naming Edition

**Naming convention rule:** IEC 61131-3 names are the canonical TOON identifiers. Generic aliases from prior versions are accepted by the parser but normalized to IEC names at parse time (Pass 0). Generators always receive the IEC canonical name.

---

## GPIO

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_DIGITAL_OUTPUT(pin)` | — | `pin: UINT` | `VOID` | `pinMode(ledPin, OUTPUT)` | `Pin(LED_PIN, Pin.OUT)` → `led_pin` global | — |
| `CFG_DIGITAL_INPUT(pin)` | — | `pin: UINT` | `VOID` | `pinMode(pin, INPUT)` | `Pin(PIN, Pin.IN)` → `pin_obj` global | — |
| `CFG_DIGITAL_INPUT_PULLUP(pin)` | — | `pin: UINT` | `VOID` | `pinMode(pin, INPUT_PULLUP)` | `Pin(PIN, Pin.IN, Pin.PULL_UP)` → `pin_obj` global | — |
| `CFG_DIGITAL_INPUT_PULLDOWN(pin)` | — | `pin: UINT` | `VOID` | `pinMode(pin, INPUT_PULLDOWN)` | `Pin(PIN, Pin.IN, Pin.PULL_DOWN)` → `pin_obj` global | `arduino-avr`: fall back to `INPUT` + `# WARN: INPUT_PULLDOWN not available on AVR` |
| `SET_BOOL(pin, value)` | `digitalWrite` | `pin: UINT, value: BOOL` | `VOID` | `digitalWrite(ledPin, value ? HIGH : LOW)` ¹ | `led_pin.value(1 if value else 0)` ¹ | Pin always resolves to label constant ² |
| `GET_BOOL(pin)` | `digitalRead` | `pin: UINT` | `BOOL` | `digitalRead(ledPin)` ¹ | `led_pin.value()` ¹ | Same label resolution as `SET_BOOL` |

**¹ Label resolution rule:** generator resolves `pin` argument to its `hardware[]` label constant (`ledPin` / `LED_PIN`). Raw integers are never emitted in output. Applies to all GPIO, PWM, ADC, and interrupt functions.

**² `SET_BOOL` platform rule:** `HIGH`/`LOW` are Arduino-only. STM32 HAL → `GPIO_PIN_SET`/`GPIO_PIN_RESET`. ESP-IDF → `1`/`0`. Generator switches on `TARGET_PLATFORM`.

**`volatile` auto-injection rule:** any `data: VAR` written inside an `isr_*` function body → C++ declares that variable as `volatile`. Parser detects this by scanning all `isr_*` bodies for assignment targets and cross-referencing `data[]`.

---

## PWM

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_PWM_OUTPUT(pin, freq)` | — | `pin: UINT, freq: UINT` | `VOID` | `arduino-esp32`: `ledcSetup(ch, freq, 16); ledcAttachPin(pin, ch)` — `arduino-avr`: `pinMode(pin, OUTPUT)` ³ | `pwm_N = PWM(Pin(PIN), freq=freq)` → global ⁴ | Allocates channel/object on first call per pin |
| `SET_PWM_FREQ(pin, hz)` | `pwmFreq` | `pin: UINT, hz: UINT` | `VOID` | `arduino-esp32`: `ledcSetup(ch, hz, 16)` — `arduino-avr`: no-op + `# WARN: runtime freq change not supported on AVR` | `pwm_N.freq(hz)` ⁴ | Triggers allocation if not already done |
| `SET_PWM_DUTY(pin, duty)` | `pwmWrite` | `pin: UINT, duty: UINT` | `VOID` | `arduino-esp32`: `ledcWrite(ch, duty)` — `arduino-avr`: `analogWrite(pin, duty >> 8)` + `# WARN: duty scaled 65535→255 for AVR` | `pwm_N.duty_u16(duty)` ⁴ | Resolves pin→channel before emit |
| `GET_PWM_DUTY(pin)` | `pwmRead` | `pin: UINT` | `UINT` | `arduino-esp32`: `ledcRead(ch)` — `arduino-avr`: `# WARN: pwmRead not available on AVR` → emit `0` | `pwm_N.duty_u16()` | — |

**³ ESP32 ledc channel rule (C++):** generator assigns `ch = 0, 1, 2...` in declaration order. Channel integers are generator-internal and never visible in TOON.

**⁴ MicroPython PWM object rule:** object named `pwm_N` where N is allocation order (0, 1, 2...). Declared globally, instantiated in `setup()`. `SET_PWM_DUTY(pin, duty)` resolves `pin→pwm_N` before emitting `pwm_N.duty_u16(duty)`.

---

## ADC

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_ADC_INPUT(pin)` | — | `pin: UINT` | `VOID` | `pinMode(pin, INPUT)` | `IMPLICIT` — ADC object handles config | — |
| `READ_ADC(pin)` | `readADC` | `pin: UINT` | `UINT` (0–65535) | `analogRead(pin)` ⁵ | `adc_N.read_u16()` ⁶ | ESP32: inject `analogReadResolution(16)` once in `setup()` — AVR: inject `map()` shim at each call site |

**⁵ ADC resolution normalization rule:**
- `arduino-esp32`: inject `analogReadResolution(16)` once in `setup()` → native 0–65535
- `arduino-avr`: inject `map(analogRead(pin), 0, 1023, 0, 65535)` at each call site
- `micropython-rp2` / `micropython-esp32`: `read_u16()` natively returns 0–65535 — no shim needed

**⁶ MicroPython ADC object rule:** `adc_N = ADC(Pin(PIN))` declared globally, instantiated in `setup()`. Inline `ADC(Pin(pin)).read_u16()` is never emitted — persistent object required on RP2040.

---

## Time

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `WAIT_TIME(t)` | `delay` | `t: TIME` | `VOID` | `delay(t_ms)` | `sleep_ms(t_ms)` | `T#` prefix and `ms` suffix stripped; C++ appends `UL` |
| `WAIT_TIME_US(t)` | — | `t: TIME` | `VOID` | `delayMicroseconds(t_us)` | `sleep_us(t_us)` | `T#` prefix and `us` suffix stripped |
| `GET_SYS_TIME()` | `currentTime` | *(none)* | `DWORD` (ms) | `millis()` | `ticks_ms()` | — |
| `GET_SYS_TIME_US()` | `currentTimeUS` | *(none)* | `DWORD` (µs) | `micros()` | `ticks_us()` | — |
| `TIME_DIFF(a, b)` | — | `a: DWORD, b: DWORD` | `DWORD` | `a - b` | `ticks_diff(a, b)` | **Always explicit** — raw subtraction of time-typed operands → `PARSE_ERROR_003` |

**Rollover rule:** raw subtraction of two `DWORD` values sourced from `GET_SYS_TIME()` or `GET_SYS_TIME_US()` is forbidden. Parser emits `PARSE_ERROR_003`. Applies to both ms and µs time domains.

---

## Math

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `SQRT(x)` | `sqrt` | `x: REAL` | `REAL` | `sqrt(x)` | `math.sqrt(x)` | Inject `#include <math.h>` / `import math` once |
| `LN(x)` | `log` | `x: REAL` | `REAL` | `log(x)` | `math.log(x)` | Natural log only. `log` alias normalized to `LN` at parse time |
| `LOG(x)` | `log10` | `x: REAL` | `REAL` | `log10(x)` | `math.log10(x)` | Base-10 only. Never conflate with `LN` |
| `ABS(x)` | `abs` | `x: ANY_NUM` | `ANY_NUM` | `abs(x)` | `abs(x)` | Inherits operand type |
| `EXPT(x, y)` | `pow` | `x: REAL, y: REAL` | `REAL` | `pow(x, y)` | `x ** y` | — |
| `FLOOR(x)` | `floor` | `x: REAL` | `INT` | `(int)floor(x)` | `math.floor(x)` | Inject `math`. Never emit `int(x // 1)` — incorrect for negatives |
| `CEIL(x)` | `ceil` | `x: REAL` | `INT` | `(int)ceil(x)` | `math.ceil(x)` | Inject `math` |
| `MAX(a, b)` | `max` | `a: T, b: T` | `T` | `max(a, b)` | `max(a, b)` | Inherits operand type |
| `MIN(a, b)` | `min` | `a: T, b: T` | `T` | `min(a, b)` | `min(a, b)` | Inherits operand type |
| `TRUNC(x)` | `int` | `x: REAL` | `INT` | `(int)(x)` | `int(x)` | Truncates toward zero |
| `TO_STRING(x)` | `string(x)` | `x: T` | `STRING` | `String(x)` | `str(x)` | — |
| `TO_STRING_FMT(x, d)` | `string(x, d)` | `x: REAL, d: UINT` | `STRING` | `String(x, d)` | `"{:.{}f}".format(x, d)` | f-string avoided for MicroPython < 1.12 |
| `CONCAT(a, b, ...)` | `"a" + string(x) + "b"` | `STRING, ...` | `STRING` | `String(a) + String(b)` | `str(a) + str(b)` | Multi-arg chains |
| `MAP_RANGE(x, in_min, in_max, out_min, out_max)` | `map_range` | `REAL×5` | `REAL` | `(x-in_min)*(out_max-out_min)/(in_max-in_min)+out_min` ⁷ | same formula | Arduino `map()` never emitted |
| `MOD(a, b)` | `%` | `a: ANY_INT, b: ANY_INT` | `ANY_INT` | `a % b` | `a % b` | — |
| `SEL(g, in0, in1)` | — | `g: BOOL, in0: T, in1: T` | `T` | `g ? in1 : in0` | `in1 if g else in0` | — |

**⁷ `MAP_RANGE` rule:** inline formula always used on all targets regardless of operand type. Arduino `map()` is integer-only and overflows on 32-bit — never emitted.

---

## Bitwise Operations

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython |
|---|---|---|---|---|---|
| `AND_BIT(a, b)` | — | `a: ANY_BIT, b: ANY_BIT` | `ANY_BIT` | `a & b` | `a & b` |
| `OR_BIT(a, b)` | — | `a: ANY_BIT, b: ANY_BIT` | `ANY_BIT` | `a \| b` | `a \| b` |
| `XOR_BIT(a, b)` | — | `a: ANY_BIT, b: ANY_BIT` | `ANY_BIT` | `a ^ b` | `a ^ b` |
| `NOT_BIT(a)` | — | `a: ANY_BIT` | `ANY_BIT` | `~a` | `~a` |
| `SHL(a, n)` | — | `a: ANY_BIT, n: UINT` | `ANY_BIT` | `a << n` | `a << n` |
| `SHR(a, n)` | — | `a: ANY_BIT, n: UINT` | `ANY_BIT` | `a >> n` | `a >> n` |
| `ROL(a, n)` | — | `a: WORD, n: UINT` | `WORD` | `(a << n) \| (a >> (16-n))` | `((a << n) \| (a >> (16-n))) & 0xFFFF` |
| `ROR(a, n)` | — | `a: WORD, n: UINT` | `WORD` | `(a >> n) \| (a << (16-n))` | `((a >> n) \| (a << (16-n))) & 0xFFFF` |

**Color constant rule:** hex literals prefixed `0x` or `16#` are typed `WORD` automatically. `16#` is IEC canonical; `0x` is accepted as alias and normalized at parse time.

---

## Type Casting

All casts are mandatory-explicit. Implicit coercion is never emitted.

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython |
|---|---|---|---|---|---|
| `REAL_TO_INT(x)` | `int(x)` | `x: REAL` | `INT` | `(int)(x)` | `int(x)` |
| `INT_TO_REAL(x)` | `float(x)` | `x: INT` | `REAL` | `(float)(x)` | `float(x)` |
| `BOOL_TO_INT(x)` | — | `x: BOOL` | `INT` | `(int)(x)` | `int(x)` |
| `INT_TO_BOOL(x)` | — | `x: INT` | `BOOL` | `(bool)(x)` | `bool(x)` |
| `WORD_TO_INT(x)` | — | `x: WORD` | `INT` | `(int)(x)` | `int(x)` |
| `INT_TO_WORD(x)` | — | `x: INT` | `WORD` | `(uint16_t)(x)` | `x & 0xFFFF` |
| `INT_TO_DINT(x)` | — | `x: INT` | `DINT` | `(long)(x)` | `int(x)` |
| `REAL_TO_LREAL(x)` | — | `x: REAL` | `LREAL` | `(double)(x)` | `float(x)` |
| `ANY_TO_STRING(x)` | `TO_STRING(x)` | `x: T` | `STRING` | `String(x)` | `str(x)` |

**Inference priority rule:** explicit cast always wins. When ambiguous, `REAL_TO_INT` is default for numeric truncation context.

---

## Boolean Logic

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython |
|---|---|---|---|---|---|
| `NOT x` | — | `x: BOOL` | `BOOL` | `!x` | `not x` |
| `AND(a, b)` | — | `a: BOOL, b: BOOL` | `BOOL` | `a && b` | `a and b` |
| `OR(a, b)` | — | `a: BOOL, b: BOOL` | `BOOL` | `a \|\| b` | `a or b` |
| `XOR(a, b)` | — | `a: BOOL, b: BOOL` | `BOOL` | `a ^ b` | `a ^ b` |

**`F_TOGGLE_BOOL` rule:** not a stdlib function. If defined in `functions:`, emitted as a normal user function. The stdlib does not reserve or inject it.

---

## Control Flow

| TOON (IEC canonical) | Alias | Signature | C++ | MicroPython |
|---|---|---|---|---|
| `IF cond:` | `if cond:` | `cond: BOOL` | `if (cond) {` | `if cond:` |
| `ELSE:` | `else:` | — | `} else {` | `else:` |
| `ELSIF cond:` | `elif cond:` | `cond: BOOL` | `} else if (cond) {` | `elif cond:` |
| `IF NOT cond:` | `if not cond:` | `cond: BOOL` | `if (!cond) {` | `if not cond:` |
| `WHILE cond:` | `while cond:` | `cond: BOOL` | `while (cond) {` | `while cond:` |
| `WHILE TRUE:` | — | — | `while (true) {` | `while True:` |
| `FOR _iN := 0 TO n:` | `repeat(n):` | `n: UINT\|INT` | `for (int _i0=0; _i0<n; _i0++) {` ⁸ | `for _i0 in range(n):` ⁸ |
| `EXIT` | `break` | — | `break;` | `break` |
| `CONTINUE` | `continue` | — | `continue;` | `continue` |
| `RETURN x` | — | `x: T` | `return x;` | `return x` |
| `RETURN NOT x` | — | `x: BOOL` | `return !x;` | `return not x` |

**⁸ `FOR` scoping rule:** loop variable uses depth-indexed naming `_i0`, `_i1`, `_i2`... for nested blocks. Generator-internal — never visible in TOON. Use explicit `data: VAR` counter if index access is needed.

---

## Interrupts

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_ATTACH_INTERRUPT(pin, fn, mode)` | `attachInterrupt` | `pin: UINT, fn: ISR_REF, mode: EDGE` | `VOID` | `attachInterrupt(digitalPinToInterrupt(pin), fn, mode)` | `Pin(pin).irq(trigger=EDGE_MAP[mode], handler=fn)` | `arduino-esp32`: inject `IRAM_ATTR` on ISR — `arduino-avr`: no attribute |
| `CFG_DETACH_INTERRUPT(pin)` | `detachInterrupt` | `pin: UINT` | `VOID` | `detachInterrupt(pin)` | `Pin(pin).irq(handler=None)` | — |

**`EDGE` enum mapping:**

| TOON | C++ | MicroPython |
|---|---|---|
| `RISING` | `RISING` | `Pin.IRQ_RISING` |
| `FALLING` | `FALLING` | `Pin.IRQ_FALLING` |
| `CHANGE` | `CHANGE` | `Pin.IRQ_RISING \| Pin.IRQ_FALLING` |

**ISR heap rule (MicroPython):** any `isr_*` function body containing `string()`, list operations, or heap-allocating calls → emit `# WARN: heap allocation forbidden in MicroPython ISR`. Recommend `micropython.schedule(fn, arg)` for deferred non-trivial work.

**`IRAM_ATTR` platform guard:** only injected on `arduino-esp32`. On `arduino-avr`, standard `attachInterrupt` with no attribute. On bare-metal targets → `VENDOR_EXTENSION("ISR_ATTRIBUTE:<target>")`.

---

## Serial / UART

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_SERIAL(baud)` | `serialBegin` | `baud: DINT` | `VOID` | `Serial.begin(baud)` | `uart_N = UART(bus, baud, tx=pin?)` → global ⁹ | Non-default `uart-tx` pin → inject `tx=Pin(pin)` |
| `SERIAL_PRINT(msg)` | `serialPrint` | `msg: STRING` | `VOID` | `Serial.print(msg)` | `uart_N.write(msg)` | — |
| `SERIAL_PRINTLN(msg)` | `serialPrintln` | `msg: STRING` | `VOID` | `Serial.println(msg)` | `uart_N.write(msg + '\n')` | — |
| `SERIAL_READLN()` | `serialReadln` | *(none)* | `STRING` | `Serial.readStringUntil('\n')` | `uart_N.readline()` | — |
| `SERIAL_READ()` | `serialRead` | *(none)* | `STRING` | `Serial.read()` | `uart_N.read(1)` | — |
| `SERIAL_AVAILABLE()` | `serialAvailable` | *(none)* | `UINT` | `Serial.available()` | `uart_N.any()` | — |
| `SERIAL_FLUSH()` | `serialFlush` | *(none)* | `VOID` | `Serial.flush()` | `uart_N.flush()` | — |

**⁹ Multi-UART rule:** multiple `uart-tx` pins declared in `hardware[]` → allocate `uart_0`, `uart_1`... in declaration order. C++ emits `Serial`, `Serial1`, `Serial2`... accordingly.

---

## I2C

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_I2C(bus, sda, scl, freq)` | `initI2C` | `bus: UINT, sda: UINT, scl: UINT, freq: DINT` | `VOID` | `Wire.begin(sda, scl)` | `i2c_N = I2C(bus, sda=Pin(sda), scl=Pin(scl), freq=freq)` → global | Declare globally; instantiate in `setup()` |
| `CFG_SENSOR_I2C()` | `initSensorI2C` | *(none)* | `VOID` | REGEX → driver `#include` | REGEX → driver `import` | No match → `VENDOR_EXTENSION("UNRESOLVED_SENSOR_I2C")` + `# WARN` |

---

## SPI

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_SPI(bus, mosi, miso, sck, cs)` | `initSPI` | `bus: UINT, mosi: UINT, miso: INT, sck: UINT, cs: UINT` | `VOID` | `SPI.begin(sck, miso, mosi, cs)` | `spi_N = SPI(bus, mosi=Pin(mosi), miso=Pin(miso), sck=Pin(sck))` → global | `miso = -1` → write-only: omit MISO from both targets |

---

## Display — TFT

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_TFT(dc, rst, cs)` | `initTFT` | `dc: UINT, rst: INT, cs: INT` | `VOID` | REGEX → `TFT_eSPI` (`arduino-esp32`) / `Adafruit_ST7789` (others) ¹⁰ | `tft = st7789.ST7789(...)` → global | `rst = -1` → omit reset pin |
| `TFT_FILL(color)` | `tftFillScreen` | `color: WORD` | `VOID` | `tft.fillScreen(color)` | `tft.fill(color)` | — |
| `TFT_SET_TEXT_COLOR(fg, bg)` | `tftSetTextColor` | `fg: WORD, bg: WORD` | `VOID` | `tft.setTextColor(fg, bg)` | `tft.color(fg)` | — |
| `TFT_SET_TEXT_SIZE(n)` | `tftSetTextSize` | `n: UINT` | `VOID` | `tft.setTextSize(n)` | scale deferred to draw call | — |
| `TFT_PRINT(x, y, msg)` | `tftPrintText` | `x: UINT, y: UINT, msg: STRING` | `VOID` | `tft.setCursor(x, y); tft.print(msg)` | `tft.text(msg, x, y)` | `CONCAT` rule applies at all `STRING` argument sites |
| `TFT_DRAW_ROUND_RECT(x, y, w, h, r, color)` | `tftDrawRoundRect` | `x,y,w,h,r: UINT, color: WORD` | `VOID` | `tft.drawRoundRect(x, y, w, h, r, color)` | `tft.rect(x, y, w, h, color)` + `# WARN` ¹¹ | — |

**¹⁰ `CFG_TFT` driver rule:** `arduino-esp32` → `TFT_eSPI`. All other targets → `Adafruit_ST7789`. REGEX failure → `VENDOR_EXTENSION("UNRESOLVED_TFT_DRIVER")` + `# WARN`.

**¹¹ `TFT_DRAW_ROUND_RECT` MicroPython fallback:** emits `tft.rect()` as plain rect + `# WARN: drawRoundRect not natively supported — emitting plain rect`. Optional: inject `_draw_round_rect()` helper if target driver supports arc primitives.

---

## Display — LCD (I2C)

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `CFG_LCD(addr, cols, rows)` | `initLCD` | `addr: BYTE, cols: UINT, rows: UINT` | `VOID` | `LiquidCrystal_I2C lcd(addr, cols, rows); lcd.init(); lcd.backlight()` | `lcd = I2cLcd(i2c, addr, rows, cols)` → global | Declare globally; instantiate in `setup()` |
| `LCD_CLEAR()` | `lcdClear` | *(none)* | `VOID` | `lcd.clear()` | `lcd.clear()` | — |
| `LCD_PRINT(col, row, msg)` | `lcdPrint` | `col: UINT, row: UINT, msg: STRING` | `VOID` | `lcd.setCursor(col, row); lcd.print(msg)` | `lcd.move_to(col, row); lcd.putstr(msg)` | `CONCAT` rule applies |

---

## Sensor FB (REGEX-resolved)

| TOON (IEC canonical) | Alias | Signature | Returns | Resolution | Failure |
|---|---|---|---|---|---|
| `CFG_SENSOR_I2C()` | `initSensorI2C` | *(none)* | `VOID` | REGEX on `hardware[]` label + `target_role` | `VENDOR_EXTENSION("UNRESOLVED_SENSOR_I2C")` + `# WARN` |
| `READ_TEMPERATURE()` | `readTemperature` | *(none)* | `REAL` | REGEX → `dht.temperature()` / `sht.temperature()` / `bme.temperature` | `VENDOR_EXTENSION("UNRESOLVED_SENSOR_FB:READ_TEMPERATURE")` + `# WARN` |
| `READ_HUMIDITY()` | `readHumidity` | *(none)* | `REAL` | REGEX → `dht.humidity()` / `sht.humidity()` / `bme.humidity` | `VENDOR_EXTENSION("UNRESOLVED_SENSOR_FB:READ_HUMIDITY")` + `# WARN` |
| `READ_PRESSURE()` | `readPressure` | *(none)* | `REAL` | REGEX → `bme.pressure` / `bmp.pressure()` | `VENDOR_EXTENSION("UNRESOLVED_SENSOR_FB:READ_PRESSURE")` + `# WARN` |

---

## Memory

| TOON (IEC canonical) | Alias | Signature | Returns | C++ | MicroPython | Trigger |
|---|---|---|---|---|---|---|
| `SYS_LOAD_MEMORY_SECTOR(RETAIN_SECTOR)` | — | *(none)* | `VOID` | `EEPROM.begin(SIZE)` ¹² | `import ujson` + load from flash file | Auto-injected at top of `setup()` if any `RETAIN` variable exists |
| `SYS_LOAD_MEMORY_SECTOR(PERSISTENT_SECTOR)` | — | *(none)* | `VOID` | `Preferences.begin("persist", false)` | `import btree` + flash mount | Auto-injected at top of `setup()` if any `PERSISTENT` variable exists |
| `SYS_WRITE_RETAIN(name, value)` | — | `name: VAR_REF, value: T` | `VOID` | `EEPROM.put(offset, value); EEPROM.commit()` ¹² | `nvs["key"] = value; flush()` | Auto-injected at each write site for `RETAIN`/`PERSISTENT` vars |
| `SYS_READ_RETAIN(name)` | — | `name: VAR_REF` | `T` | `EEPROM.get(offset, var)` ¹² | `var = nvs.get("key", default)` | Auto-injected at each read site for `RETAIN`/`PERSISTENT` vars |

**¹² EEPROM SIZE rule (C++):** generator calculates `SIZE` at codegen time as `sum(sizeof(T))` for all `data[]` variables with scope `RETAIN` or `PERSISTENT`. `offset` per variable is the cumulative byte offset in declaration order.

---

## VENDOR_EXTENSION (Formal Specification)

`VENDOR_EXTENSION` is a first-class TOON construct emitted by the generator when a function cannot be resolved to a known stdlib mapping.

```
VENDOR_EXTENSION("<CATEGORY:DETAIL>")
# WARN: <human-readable description>
```

**Rules:**
- Never silently skip an unresolved function — always emit `VENDOR_EXTENSION`
- `REASON` format: `"CATEGORY:DETAIL"` e.g. `"UNRESOLVED_SENSOR_FB:READ_TEMPERATURE"`, `"ISR_ATTRIBUTE:bare-metal-stm32"`
- Generator must halt codegen for the affected block and resume at the next statement
- Consumer tools must treat any `VENDOR_EXTENSION` as a mandatory manual review point

---

## Cross-Cutting Generator Rules

Rules that span multiple sections and are not co-located with a single function.

```
NAMING CONVENTIONS
  Hardware pin constant    → C++: camelCase (ledPin)     MicroPython: UPPER_SNAKE (LED_PIN)
  Hardware Pin object      → MicroPython: lower_snake (led_pin)
  User functions           → C++: preserved case          MicroPython: lower_snake
  data: CONST              → UPPER_SNAKE on all targets
  data: VAR                → C++: camelCase               MicroPython: lower_snake
  IEC 16# hex literals     → normalized to 0x at codegen for C++ and MicroPython

GPIO
  Any data: VAR written inside isr_* body →
  C++: declare that variable volatile

PERIPHERAL OBJECTS (MicroPython)
  CFG_I2C / CFG_SPI / CFG_SERIAL / CFG_TFT / CFG_LCD →
  declare named global object before setup()
  names: i2c_N, spi_N, uart_N, tft, lcd
  instantiate inside setup()

MEMORY
  Any data: RETAIN  → inject SYS_LOAD_MEMORY_SECTOR(RETAIN_SECTOR) at top of setup()
                      inject SYS_WRITE_RETAIN / SYS_READ_RETAIN at each write/read site
  Any data: PERSISTENT → inject SYS_LOAD_MEMORY_SECTOR(PERSISTENT_SECTOR) at top of setup()

ENTRY POINT (MicroPython only)
  setup()
  while True:
      loop()

PLATFORM FLAG
  TARGET_PLATFORM ∈ {
    arduino-avr | arduino-esp32 | micropython-esp32 |
    micropython-rp2 | bare-metal-stm32 | codesys | tia-portal
  }

UNRESOLVED FALLBACK
  Any unresolved function → VENDOR_EXTENSION("CATEGORY:DETAIL") + # WARN
  Never silently skip
```
