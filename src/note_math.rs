use std::{
    ops::Range,
    time::{Duration, Instant},
};

use fend_core::{Context, ExchangeRateFnV2, ExchangeRateFnV2Options, FendResult, Interrupt};
use gpui_kit::component::input::{Rope, RopeExt};

use crate::list_edit::FencedCode;
use crate::note_edit::{TextEdit, comment_offset, line_ending};

const EXCHANGE_RATES: &str = include_str!("../assets/ecb-reference-rates.txt");
const MAX_EXPRESSION_BYTES: usize = 512;
const MAX_EVALUATION_TIME: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MathTrigger {
    Equals,
    Return,
}

pub(crate) fn math_edit(
    text: &Rope,
    selection: Range<usize>,
    trigger: MathTrigger,
) -> Option<TextEdit> {
    let row = text.offset_to_point(selection.start).row;
    let line_start = text.line_start_offset(row);
    let line_end = text.line_end_offset(row);
    if selection.end > line_end {
        return None;
    }
    let line = text.slice(line_start..line_end).to_string();
    let uncommented = before_comment(&line);
    let code = uncommented
        .split_once(';')
        .map_or(uncommented, |(code, _)| code);
    let (input, end) = match trigger {
        MathTrigger::Equals => {
            let start = selection.start - line_start;
            let end = selection.end - line_start;
            if end > code.len() {
                return None;
            }
            let rest = code[end..].trim();
            if !rest.is_empty() && !rest.starts_with('=') {
                return None;
            }
            let end = if code.len() == line.len() {
                line_end
            } else {
                line_start + code.trim_end().len()
            };
            (&code[..start], end)
        }
        MathTrigger::Return => {
            if selection != (line_end..line_end) || code.len() != uncommented.len() {
                return None;
            }
            let input = match code.split_once('=') {
                Some((input, result)) if result.trim().is_empty() => input,
                Some(_) => return None,
                None if is_conversion(code) => code,
                None => return None,
            };
            (input, line_end)
        }
    };
    let statement = Statement::parse(input)?;
    let preceding = text.slice(0..line_start).to_string();
    if !preceding.contains("math:") {
        return None;
    }
    let fenced = FencedCode::parse(text);
    if fenced.contains(line_start) {
        return None;
    }
    let block_start = block_start(&preceding, &fenced)?;
    let mut calculator = calculator_context();
    let deadline = Deadline(Instant::now() + MAX_EVALUATION_TIME);

    let lines = preceding[block_start..].split_inclusive('\n');
    let lines = lines.scan(block_start, |offset, line| {
        let start = *offset;
        *offset += line.len();
        Some((start, line))
    });
    for (offset, line) in lines {
        if deadline.should_interrupt() {
            return None;
        }
        if fenced.contains(offset) {
            continue;
        }
        if let Some((input, _)) = before_comment(line).split_once('=')
            && let Some(binding) = Statement::parse(input)
            && binding.name.is_some()
        {
            binding.evaluate(&mut calculator, &deadline);
        }
    }

    let result = statement.evaluate(&mut calculator, &deadline)?;
    let answer = display_result(result, statement.expression, &mut calculator, &deadline);
    let start = line_start + input.trim_end().len();
    let mut replacement = format!(" = {answer}");
    if trigger == MathTrigger::Return {
        replacement.push_str(&line[code.trim_end().len()..]);
        replacement.push_str(line_ending(text, row));
    }
    let caret = start + replacement.len();
    Some(TextEdit {
        range: start..end,
        replacement,
        selection_after: caret..caret,
    })
}

fn block_start(preceding: &str, fenced: &FencedCode) -> Option<usize> {
    let mut start = None;
    let mut offset = 0;
    for line in preceding.split_inclusive('\n') {
        let end = offset + line.len();
        if !fenced.contains(offset) {
            if line.trim() == "math:" {
                start = Some(end);
            } else if before_comment(line).contains(';') {
                start = None;
            }
        }
        offset = end;
    }
    start
}

fn before_comment(line: &str) -> &str {
    &line[..comment_offset(line.bytes()).unwrap_or(line.len())]
}

pub(crate) struct ResultSpan {
    pub(crate) range: Range<usize>,
    pub(crate) equals: usize,
    pub(crate) line_end: usize,
}

pub(crate) fn math_results(
    text: &Rope,
    tracked_markers: &[usize],
) -> Vec<(ResultSpan, Option<String>)> {
    let source = text.to_string();
    if !source.contains("math:") {
        return Vec::new();
    }
    let fenced = FencedCode::parse(text);
    let mut calculator = None;
    let mut offset = 0;
    let mut results = Vec::new();
    for line in source.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let line = line.trim_end_matches(['\r', '\n']);
        if fenced.contains(start) {
            continue;
        }
        if line.trim() == "math:" {
            calculator = Some(calculator_context());
            continue;
        }
        let uncommented = before_comment(line);
        let (code, ends_block) = uncommented
            .split_once(';')
            .map_or((uncommented, false), |(code, _)| (code, true));
        if let Some(calculator) = &mut calculator
            && let Some((input, old_result)) = code.split_once('=')
        {
            let equals = start + input.len();
            let deadline = Deadline(Instant::now() + MAX_EVALUATION_TIME);
            let result = Statement::parse(input).and_then(|statement| {
                let result = statement.evaluate(calculator, &deadline)?;
                Some(display_result(
                    result,
                    statement.expression,
                    calculator,
                    &deadline,
                ))
            });
            if result.is_some() || old_result.trim() == "?" || tracked_markers.contains(&equals) {
                let suffix_start = input.trim_end().len();
                let suffix_end = if code.len() == line.len() {
                    code.len()
                } else {
                    code.trim_end().len()
                };
                let suffix = format!(" = {}", result.as_deref().unwrap_or("?"));
                let replacement = (code[suffix_start..suffix_end] != suffix).then_some(suffix);
                results.push((
                    ResultSpan {
                        range: start + suffix_start..start + suffix_end,
                        equals,
                        line_end: start + line.len(),
                    },
                    replacement,
                ));
            }
        }
        if ends_block {
            calculator = None;
        }
    }
    results
}

fn calculator_context() -> Context {
    let mut calculator = Context::new();
    calculator.disable_rng();
    calculator.set_exchange_rate_handler_v2(ReferenceRates);
    calculator
}

fn is_conversion(expression: &str) -> bool {
    let words = expression.split_whitespace();
    words
        .clone()
        .skip(1)
        .zip(words.skip(2))
        .any(|(word, unit)| {
            matches!(word, "in" | "to")
                && unit.starts_with(|ch: char| ch.is_alphabetic() || "$€£°".contains(ch))
        })
}

struct Statement<'a> {
    name: Option<&'a str>,
    expression: &'a str,
}

impl<'a> Statement<'a> {
    fn parse(input: &'a str) -> Option<Self> {
        let (name, expression) = match input.split_once(':') {
            Some((name, expression)) => {
                let name = name.trim();
                let mut chars = name.chars();
                if name.len() > MAX_EXPRESSION_BYTES
                    || !chars.next()?.is_ascii_alphabetic()
                    || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                {
                    return None;
                }
                (Some(name), expression)
            }
            None => (None, input),
        };
        let expression = expression.trim();
        if expression.is_empty()
            || expression.len() > MAX_EXPRESSION_BYTES
            || expression.contains(['=', ';'])
        {
            return None;
        }
        Some(Self { name, expression })
    }

    fn evaluate(&self, calculator: &mut Context, deadline: &Deadline) -> Option<FendResult> {
        let assignment;
        let expression = if let Some(name) = self.name {
            assignment = format!("{name} = ({})", self.expression);
            &assignment
        } else {
            self.expression
        };
        let result = fend_core::evaluate_with_interrupt(expression, calculator, deadline).ok()?;
        let output = result.get_main_result();
        (output.len() <= MAX_EXPRESSION_BYTES
            && !output.contains(['\r', '\n'])
            && result
                .get_main_result_spans()
                .any(|part| part.kind() == fend_core::SpanKind::Number))
        .then_some(result)
    }
}

fn display_result(
    mut result: FendResult,
    expression: &str,
    calculator: &mut Context,
    deadline: &Deadline,
) -> String {
    let conversion = is_conversion(expression);
    let raw = result.get_main_result();
    let currency = raw.split_whitespace().last().is_some_and(|unit| {
        exchange_rate(unit).is_some() || matches!(unit, "dollar" | "dollars" | "euro" | "euros")
    }) || raw.contains(['$', '€', '£']);
    if conversion
        && currency
        && let Ok(rounded) = fend_core::evaluate_with_interrupt("ans to 2 dp", calculator, deadline)
    {
        result = rounded;
    }
    let raw = result.get_main_result();
    let value = raw.strip_prefix("approx. ").unwrap_or(raw);
    for (unit, symbol) in [(" EUR", "€"), (" USD", "$"), (" GBP", "£")] {
        if let Some(amount) = value.strip_suffix(unit) {
            return format!("{amount}{symbol}");
        }
    }
    if value.parse::<i128>().is_ok() || currency {
        value.to_owned()
    } else {
        raw.to_owned()
    }
}

struct Deadline(Instant);

impl Interrupt for Deadline {
    fn should_interrupt(&self) -> bool {
        Instant::now() >= self.0
    }
}

struct ReferenceRates;

impl ExchangeRateFnV2 for ReferenceRates {
    fn relative_to_base_currency(
        &self,
        currency: &str,
        _: &ExchangeRateFnV2Options,
    ) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
        exchange_rate(currency)
            .ok_or_else(|| "currency unavailable in the bundled ECB rates".into())
    }
}

fn exchange_rate(currency: &str) -> Option<f64> {
    EXCHANGE_RATES
        .lines()
        .filter_map(|line| line.split_once(' '))
        .find_map(|(code, rate)| (code == currency).then(|| rate.parse().ok()).flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press_at_end(note: &str, trigger: MathTrigger) -> Option<String> {
        let caret = note.len();
        let edit = math_edit(&Rope::from(note), caret..caret, trigger)?;
        let mut result = note.to_owned();
        result.replace_range(edit.range, &edit.replacement);
        assert_eq!(edit.selection_after, result.len()..result.len());
        Some(result)
    }

    #[test]
    fn evaluates_arithmetic_functions_and_conversions() {
        for (input, trigger, output) in [
            ("5 + 3 ", MathTrigger::Equals, "5 + 3 = 8"),
            ("log2 8", MathTrigger::Equals, "log2 8 = 3"),
            ("2 ^ (3 + 1) =", MathTrigger::Return, "2 ^ (3 + 1) = 16\n"),
            ("5 eur * 2", MathTrigger::Equals, "5 eur * 2 = 10€"),
            ("5 km in m", MathTrigger::Return, "5 km in m = 5000 m\n"),
            (
                "20 eur in usd",
                MathTrigger::Return,
                "20 eur in usd = 21.91$\n",
            ),
        ] {
            let note = format!("📝 Plain text\n\nmath:\n{input}");
            assert_eq!(
                press_at_end(&note, trigger),
                Some(format!("📝 Plain text\n\nmath:\n{output}"))
            );
        }
    }

    #[test]
    fn replays_assignments_from_source_within_each_block() {
        let block = "math:\nx: 5.2 * 5 = 26\nx: x + 3 = 30\n// x: 99 = 99;\nplain text";
        let note = format!("{block}\nx + 2");
        assert_eq!(
            press_at_end(&note, MathTrigger::Equals),
            Some(format!("{note} = 31"))
        );
        let note = format!("{block}\n;\nmath:\nx + 2");
        assert_eq!(press_at_end(&note, MathTrigger::Equals), None);
        let note = format!("{block}\nmath:\nx + 2");
        assert_eq!(press_at_end(&note, MathTrigger::Equals), None);
    }

    #[test]
    fn preserves_plain_text_and_code_examples() {
        for note in [
            "5 + 3",
            "math:\n;\nlog2 8",
            "math:\n5 + 3 = 8;\nlog2 8",
            "math:\nFollow up with the team",
            "math:\n// 5 + 3",
            "math:\n1 / 0",
            "math:\n```\n5 + 3",
            "```\nmath:\n```\n5 + 3",
        ] {
            assert_eq!(press_at_end(note, MathTrigger::Equals), None, "{note}");
        }
        assert_eq!(press_at_end("math:\n5 + 3", MathTrigger::Return), None);
        assert_eq!(press_at_end("math:\n5 + 3 = 8", MathTrigger::Return), None);
    }

    #[test]
    fn updates_an_existing_result_and_keeps_the_inline_comment() {
        let note = "math:\n8 * 2 = 8  // keep this; café\n";
        let caret = note.find('=').unwrap();
        let edit = math_edit(&Rope::from(note), caret..caret, MathTrigger::Equals).unwrap();
        let mut result = note.to_owned();
        result.replace_range(edit.range, &edit.replacement);
        assert_eq!(result, "math:\n8 * 2 = 16  // keep this; café\n");
        assert_eq!(edit.selection_after.start, result.find("  //").unwrap());

        assert_eq!(
            press_at_end("math:\n5 km in m  // café", MathTrigger::Return),
            Some("math:\n5 km in m = 5000 m  // café\n".into())
        );
        assert_eq!(
            press_at_end("math:\r\n5 km in m", MathTrigger::Return),
            Some("math:\r\n5 km in m = 5000 m\r\n".into())
        );
    }

    #[test]
    fn keeps_the_terminator_when_refreshing_a_result() {
        let note = "math:\n8 * 2 = 8;\nplain text";
        let caret = note.find('=').unwrap();
        let edit = math_edit(&Rope::from(note), caret..caret, MathTrigger::Equals).unwrap();
        let mut result = note.to_owned();
        result.replace_range(edit.range, &edit.replacement);
        assert_eq!(result, "math:\n8 * 2 = 16;\nplain text");
    }
}
