//! Certified board pin/port reconciliation and AVR register/intrinsic tables (`docs/lbc/cpp.md` §6).
//!
//! Pure in-memory certified `arduino-uno-r3` (ATmega328P) port table (zero compile-time filesystem reads):
//! capabilities match `assets/boards/arduino-uno-r3/profile.toml` verbatim.

use dendri_ast::{ChannelRange, SignalKind};
use dendri_frontend_core::BoardContext;

/// Certified PWM-capable pins on `arduino-uno-r3` (ATmega328P Timers 0/1/2).
const UNO_PWM_PINS: &[&str] = &["D3", "D5", "D6", "D9", "D10", "D11"];

/// Certified hardware external-interrupt pins on `arduino-uno-r3` (`INT0` / `INT1`).
const UNO_INT_PINS: &[&str] = &["D2", "D3"];

/// Certified analog-input pins on `arduino-uno-r3` (`ADC0..ADC5`).
const UNO_ANALOG_PINS: &[&str] = &["A0", "A1", "A2", "A3", "A4", "A5"];

/// Certified digital pins on `arduino-uno-r3` (`D0..D13`).
const UNO_DIGITAL_PINS: &[&str] = &[
    "D0", "D1", "D2", "D3", "D4", "D5", "D6", "D7", "D8", "D9", "D10", "D11", "D12", "D13",
];

/// Direct AVR / ATmega328P MCU register identifiers preserved verbatim as `Foreign(F0)`.
const AVR_REGISTERS: &[&str] = &[
    "PORTA", "PORTB", "PORTC", "PORTD", "PORTE", "PORTF", "PORTG", "PORTH", "PORTJ", "PORTK",
    "PORTL", "DDRA", "DDRB", "DDRC", "DDRD", "DDRE", "DDRF", "DDRG", "DDRH", "DDRJ", "DDRK",
    "DDRL", "PINA", "PINB", "PINC", "PIND", "PINE", "PINF", "PING", "PINH", "PINJ", "PINK", "PINL",
    "TCCR0A", "TCCR0B", "TCNT0", "OCR0A", "OCR0B", "TIMSK0", "TIFR0", "TCCR1A", "TCCR1B", "TCCR1C",
    "TCNT1", "TCNT1H", "TCNT1L", "OCR1A", "OCR1B", "ICR1", "TIMSK1", "TIFR1", "TCCR2A", "TCCR2B",
    "TCNT2", "OCR2A", "OCR2B", "TIMSK2", "TIFR2", "ADMUX", "ADCSRA", "ADCSRB", "ADCL", "ADCH",
    "ADC", "DIDR0", "UCSR0A", "UCSR0B", "UCSR0C", "UBRR0H", "UBRR0L", "UDR0", "SPCR", "SPSR",
    "SPDR", "TWBR", "TWSR", "TWAR", "TWDR", "TWCR", "EICRA", "EIMSK", "EIFR", "PCICR", "PCMSK0",
    "PCMSK1", "PCMSK2", "WDTCSR", "SREG", "MCUCR", "MCUSR", "CLKPR", "PRR", "SMCR",
];

/// Known Arduino/AVR compiler & hardware vendor intrinsics preserved as `Foreign(F0)`.
const VENDOR_INTRINSICS: &[&str] = &[
    "_BV",
    "bitRead",
    "bitSet",
    "bitClear",
    "bitWrite",
    "lowByte",
    "highByte",
    "cli",
    "sei",
    "wdt_reset",
    "wdt_enable",
    "wdt_disable",
    "noInterrupts",
    "interrupts",
    "F",
    "tone",
    "noTone",
    "pulseIn",
    "shiftOut",
    "shiftIn",
    "analogReference",
    "yield",
    "sizeof",
    "offsetof",
];

/// Returns `true` iff `ctx` targets the certified `arduino-uno-r3` board profile.
#[must_use]
pub(crate) fn is_arduino_uno_r3(ctx: &BoardContext) -> bool {
    ctx.profile_id == "arduino-uno-r3"
}

/// Returns `true` iff `ident` is a direct AVR/MCU register name (`PORTB`, `DDRB`, `TCCR0A`, etc.).
#[must_use]
pub(crate) fn is_avr_register(ident: &str) -> bool {
    AVR_REGISTERS.contains(&ident)
}

/// Returns `true` iff `ident` is a vendor/compiler intrinsic preserved as `Foreign(F0)`.
#[must_use]
pub(crate) fn is_vendor_intrinsic(ident: &str) -> bool {
    ident.starts_with("__builtin_")
        || ident.starts_with("__avr_")
        || ident.starts_with("__asm")
        || ident.starts_with("pgm_read_")
        || VENDOR_INTRINSICS.contains(&ident)
}

/// Returns `true` iff `ident` is a built-in Arduino pin identifier (`A0..A5`, `LED_BUILTIN`).
#[must_use]
pub(crate) fn is_builtin_pin_ident(ident: &str) -> bool {
    ident == "LED_BUILTIN" || UNO_ANALOG_PINS.contains(&ident)
}

/// Resolves an integer pin literal (`0..=13` on Uno R3, `14..=19` -> `A0..A5`) to a canonical port ID.
#[must_use]
pub(crate) fn resolve_pin_from_int(n: i128, ctx: &BoardContext) -> Option<String> {
    if n < 0 {
        return None;
    }
    if is_arduino_uno_r3(ctx) {
        if (0..=13).contains(&n) {
            return Some(format!("D{n}"));
        }
        if (14..=19).contains(&n) {
            return Some(format!("A{}", n - 14));
        }
        return None;
    }
    if !ctx.port_to_iec.is_empty() {
        // PLC boards do not use bare Arduino pin integers unless matching a registered port.
        let candidate = format!("D{n}");
        if ctx.port_to_iec.contains_key(&candidate) {
            return Some(candidate);
        }
        return None;
    }
    if n <= 63 { Some(format!("D{n}")) } else { None }
}

/// Resolves a symbolic pin identifier (`A0..A5`, `LED_BUILTIN`, `D0..D13`, or PLC port IDs) to a canonical port ID.
#[must_use]
pub(crate) fn resolve_pin_from_ident(ident: &str, ctx: &BoardContext) -> Option<String> {
    if ident == "LED_BUILTIN" {
        return if is_known_port("D13", ctx) {
            Some("D13".to_owned())
        } else {
            None
        };
    }
    if is_known_port(ident, ctx) {
        return Some(ident.to_owned());
    }
    // Support sanitized PLC port identifiers like `Ia_0` -> `Ia.0` when registered in `port_to_iec`.
    if let Some(port) = ctx
        .port_to_iec
        .keys()
        .find(|p| sanitize_port_ident(p) == ident)
    {
        return Some(port.clone());
    }
    None
}

/// Returns `true` iff `port` exists on the active board profile.
#[must_use]
pub(crate) fn is_known_port(port: &str, ctx: &BoardContext) -> bool {
    if is_arduino_uno_r3(ctx) {
        return UNO_DIGITAL_PINS.contains(&port) || UNO_ANALOG_PINS.contains(&port);
    }
    if !ctx.port_to_iec.is_empty() {
        return ctx.port_to_iec.contains_key(port);
    }
    // Generic MCU context (e.g. 32-bit targets)
    if let Some(rest) = port.strip_prefix('D')
        && let Ok(n) = rest.parse::<u32>()
    {
        return n <= 63;
    }
    if let Some(rest) = port.strip_prefix('A')
        && let Ok(n) = rest.parse::<u32>()
    {
        return n <= 15;
    }
    false
}

/// Returns `true` iff `port` supports PWM output (`analogWrite`).
#[must_use]
pub(crate) fn supports_pwm(port: &str, ctx: &BoardContext) -> bool {
    if is_arduino_uno_r3(ctx) {
        return UNO_PWM_PINS.contains(&port);
    }
    if !ctx.port_to_iec.is_empty() {
        return port.starts_with("Qa.") || port.starts_with("QW");
    }
    port.starts_with('D') && is_known_port(port, ctx)
}

/// Returns `true` iff `port` supports analog input (`analogRead`).
#[must_use]
pub(crate) fn supports_analog_in(port: &str, ctx: &BoardContext) -> bool {
    if is_arduino_uno_r3(ctx) {
        return UNO_ANALOG_PINS.contains(&port);
    }
    if !ctx.port_to_iec.is_empty() {
        return port.starts_with("IW");
    }
    port.starts_with('A') && is_known_port(port, ctx)
}

/// Returns `true` iff `port` supports external hardware interrupts (`attachInterrupt`).
#[must_use]
pub(crate) fn supports_interrupt(port: &str, ctx: &BoardContext) -> bool {
    if is_arduino_uno_r3(ctx) {
        return UNO_INT_PINS.contains(&port);
    }
    is_known_port(port, ctx)
}

/// Default physical [`ChannelRange`] for analog inputs on `ctx`.
#[must_use]
pub(crate) fn default_analog_range(ctx: &BoardContext) -> ChannelRange {
    let high = if is_arduino_uno_r3(ctx) {
        5.0
    } else if ctx.profile_id.starts_with("plc-") {
        10.0
    } else {
        3.3
    };
    ChannelRange {
        signal: SignalKind::Voltage,
        low: 0.0,
        high,
        live_zero: false,
        hardware_normalizes_live_zero: false,
    }
}

/// Default ADC codec ID for `ctx`.
#[must_use]
pub(crate) fn default_adc_codec_id(ctx: &BoardContext) -> Option<String> {
    ctx.default_adc_codec
        .as_ref()
        .map(|c| c.id.clone())
        .or_else(|| {
            if is_arduino_uno_r3(ctx) {
                Some("uno_10bit".to_owned())
            } else {
                None
            }
        })
}

/// Default PWM codec ID for `ctx` (`"pwm_8bit"` when declared on the board).
#[must_use]
pub(crate) fn default_pwm_codec_id(ctx: &BoardContext) -> Option<String> {
    if ctx.codecs.contains_key("pwm_8bit") || is_arduino_uno_r3(ctx) {
        Some("pwm_8bit".to_owned())
    } else {
        ctx.codecs.keys().find(|k| k.contains("pwm")).cloned()
    }
}

/// Converts a canonical board port ID into an idiomatic C++ pin expression (`"D13"` -> `"13"`, `"A0"` -> `"A0"`).
#[must_use]
pub(crate) fn port_to_cpp_pin_expr(port: &str) -> String {
    if let Some(rest) = port.strip_prefix('D')
        && !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_digit())
    {
        return rest.to_owned();
    }
    if port.contains('.') {
        return sanitize_port_ident(port);
    }
    port.to_owned()
}

/// Sanitizes a dotted port ID (e.g. `"Ia.0"`) into a valid C++ identifier (`"Ia_0"`).
#[must_use]
pub(crate) fn sanitize_port_ident(port: &str) -> String {
    port.replace(['.', '-', '/'], "_")
}

/// Returns `true` iff `label` is the implicit default label for `pin` (`"D13"` on `"D13"`, `"A0"` on `"A0"`, `"LED_BUILTIN"` on `"D13"`).
#[must_use]
pub(crate) fn is_implicit_pin_label(label: &str, pin: &str) -> bool {
    label == pin || (label == "LED_BUILTIN" && pin == "D13")
}

/// Returns the idiomatic C++ pin reference to emit inside `pinMode`/`digitalWrite`/`analogRead`/etc.
#[must_use]
pub(crate) fn hardware_label_to_cpp_arg(label: &str, pin: &str) -> String {
    if label == pin {
        port_to_cpp_pin_expr(pin)
    } else {
        label.to_owned()
    }
}
