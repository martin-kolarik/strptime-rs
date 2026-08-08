use std::iter::Peekable;
use std::num::ParseIntError;
use std::ops::Deref;
use std::ops::DerefMut;
use std::str::Chars;
use std::str::FromStr;

use crate::ParseError;
use crate::ParseOptions;
use crate::ParseResult;
use crate::RawDateTime;
use crate::error::ErrorKind;

/// An object that parses one and exactly one date and time string, and is consumed.
#[must_use]
pub(crate) struct OnceParser<'a> {
  fmt: &'static str,
  date_str: &'a str,
  opts: ParseOptions,
  partials: Partials,
}

impl<'a> OnceParser<'a> {
  #[inline]
  pub(crate) fn new(fmt: &'static str, date_str: &'a str, opts: ParseOptions) -> Self {
    Self { fmt, date_str, opts, partials: Partials::default() }
  }

  pub(crate) fn parse(mut self) -> ParseResult<RawDateTime> {
    let mut answer = RawDateTime { src: self.date_str.into(), date: None, time: None };

    // Begin iterating over the format string, and incrementally "chew" characters from the
    // beginning of the date string.
    let mut input = Input::new(self.date_str);
    let mut flag = false;
    let mut padding = None;
    let mut nano_digits = None;
    for ch in self.fmt.chars() {
      if ch != 'f' && nano_digits.is_some() {
        input.fail(ErrorKind::InvalidFormat)?;
      }
      match flag {
        true => {
          flag = false;
          match ch {
            // Date: Year
            'Y' => answer.set_year(input.parse_int::<i16>(4, padding)?),
            'C' => self.partials.century = Some(input.parse_int::<i16>(2, padding)?),
            'y' => self.partials.year_modulo = Some(input.parse_int::<i16>(2, padding)?),
            // Date: Month
            'm' => answer.set_month(input.parse_int::<u8>(2, padding)?),
            'b' | 'h' => answer.set_month(input.parse_month_abbr()?),
            'B' => answer.set_month(input.parse_month()?),
            // Date: Day
            'd' => answer.set_day(input.parse_int::<u8>(2, padding)?),
            'e' => answer.set_day(input.parse_int::<u8>(2, Some(padding.unwrap_or(' ')))?),
            // Date: Weekday
            //
            // Currently this is just thrown away once validation is done, but once %U/%W are
            // supported, this could be used to parse a full date.
            'a' => drop(input.parse_weekday_abbr()?),
            'A' => drop(input.parse_weekday()?),
            // Time: Hour
            'H' => answer.set_hour(input.parse_int::<u8>(2, padding)?),
            'k' => answer.set_hour(input.parse_int::<u8>(2, Some(padding.unwrap_or(' ')))?),
            'I' => self.partials.hour_12 = Some(input.parse_int::<u8>(2, padding)?),
            'p' => self.partials.pm = Some(input.parse_am_pm_upper()?),
            'P' => self.partials.pm = Some(input.parse_am_pm_lower()?),
            // Time: Minute
            'M' => answer.set_minute(input.parse_int::<u8>(2, padding)?),
            // Time: Second
            'S' => answer.set_second(input.parse_int::<u8>(2, padding)?),
            // Time: Nanosecond
            'f' => match nano_digits.take() {
              Some(3) => answer.set_nanosecond(input.parse_int::<u64>(3, Some('0'))? * 1_000_000),
              Some(6) => answer.set_nanosecond(input.parse_int::<u64>(6, Some('0'))? * 1000),
              Some(9) => answer.set_nanosecond(input.parse_int::<u64>(9, Some('0'))?),
              _ => {
                let nanoseconds_fraction = input.parse_int::<u64>(9, Some('-'))?;
                let digits = 1 + nanoseconds_fraction.ilog10();
                let nanoseconds = if digits == 9 {
                  nanoseconds_fraction
                } else {
                  nanoseconds_fraction * 10u64.pow(9 - digits)
                };
                answer.set_nanosecond(nanoseconds);
              },
            },
            // Time Zone
            'z' => {
              let sign = input.parse_sign()?;
              answer.set_utc_offset(input.parse_int::<i32>(4, Some('0'))? * sign);
            },
            // Padding change modifiers.
            '-' | '0' | ' ' => {
              padding = Some(ch);
              flag = true;
            },
            // Prefix modifiers
            '.' => {
              input.expect_char('.')?;
              flag = true;
            },
            // Nanosecond modifiers
            '3' | '6' | '9' => {
              nano_digits = ch.to_digit(10);
              flag = true;
            },
            _ => input.fail(ErrorKind::InvalidFormat)?,
          }
        },
        false => match ch {
          '%' => flag = true,
          ch => input.expect_char(ch)?,
        },
      };
    }

    // Process partials.
    if let Some(year) = self.partials.year(self.date_str, &self.opts)? {
      answer.set_year(year);
    }
    if let Some(hour) = self.partials.hour(self.date_str)? {
      answer.set_hour(hour);
    }

    // Assert that our answer is complete.
    answer.assert_complete(self.date_str)?;
    input.assert_consumed()?;
    Ok(answer)
  }
}

/// A wrapper around the original input, capable of easily handling errors.
struct Input<'a> {
  src: &'a str,
  chars: Peekable<Chars<'a>>,
}

impl<'a> Input<'a> {
  fn new(date_str: &'a str) -> Self {
    Self { src: date_str, chars: date_str.chars().peekable() }
  }
}

impl<'a> Deref for Input<'a> {
  type Target = Peekable<Chars<'a>>;

  fn deref(&self) -> &Self::Target {
    &self.chars
  }
}

impl<'a> DerefMut for Input<'a> {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.chars
  }
}

impl<'a> Input<'a> {
  /// Pop characters off of the beginning and yield them.
  fn pop_front(&mut self, n: usize) -> String {
    let mut s = String::with_capacity(n);
    while s.len() < n {
      let Some(ch) = self.next() else { return s };
      s.push(ch);
    }
    s
  }

  /// Pop characters off of the beginning while they satisfy the given condition.
  fn pop_front_while(&mut self, mut pred: impl FnMut(&char) -> bool) -> String {
    let mut s = String::new();
    loop {
      match self.peek() {
        Some(ch) if !pred(ch) => break,
        None => break,
        _ => s.push(self.next().unwrap()),
      }
    }
    s
  }

  /// Parse a static character.
  fn expect_char(&mut self, ch: char) -> ParseResult<()> {
    self.expect_chars(&[ch])?;
    Ok(())
  }

  /// Parse one of an option of static characters.
  fn expect_chars(&mut self, chars: &[char]) -> ParseResult<char> {
    self.peek().cloned().ok_or_else(|| self.err(ErrorKind::InputTooShort)).and_then(
      |c| match chars.contains(&c) {
        true => {
          self.next();
          Ok(c)
        },
        false => self.fail(ErrorKind::Unexpected),
      },
    )
  }

  fn parse_sign(&mut self) -> ParseResult<i32> {
    let sign_char = self.expect_chars(&['+', '-'])?;
    Ok(match sign_char {
      '+' => 1,
      '-' => -1,
      _ => unreachable!("Only `+` or `-` can have been returned from expect_chars"),
    })
  }

  /// Parse an integer, usually with the given number of digits, from the input.
  fn parse_int<I: FromStr<Err = ParseIntError>>(
    &mut self, mut digits: usize, padding: Option<char>,
  ) -> ParseResult<I> {
    let e = self.err(ErrorKind::Unexpected);
    match padding {
      Some('-') => self
        .pop_front_while(|c| {
          if digits == 0 {
            false
          } else {
            digits -= 1;
            c.is_numeric()
          }
        })
        .parse::<I>()
        .map_err(|_| e),
      Some(' ') => self.pop_front(digits).trim_start().parse::<I>().map_err(|_| e),
      Some('0') | None => self.pop_front(digits).parse::<I>().map_err(|_| e),
      _ => unreachable!("Invalid padding"),
    }
  }

  /// Parse a month abbreviation (always three letters).
  fn parse_month_abbr(&mut self) -> ParseResult<u8> {
    let abbr = self.pop_front(3).to_lowercase();
    Ok(match abbr.as_str() {
      "jan" => 1,
      "feb" => 2,
      "mar" => 3,
      "apr" => 4,
      "may" => 5,
      "jun" => 6,
      "jul" => 7,
      "aug" => 8,
      "sep" => 9,
      "oct" => 10,
      "nov" => 11,
      "dec" => 12,
      _ => self.fail(ErrorKind::Unexpected)?,
    })
  }

  /// Parse a full month name. This succeeds if at least the three-letter abbreviation is present,
  /// and continues to parse until the month name is completed or it ceases to find a match
  /// (therefore matching "Sept", for example).
  fn parse_month(&mut self) -> ParseResult<u8> {
    let month = self.parse_month_abbr()?;
    self.trim_front_seq(match month {
      1 => "uary",
      2 => "ruary",
      3 => "ch",
      4 => "il",
      6 => "e",
      7 => "y",
      8 => "ust",
      9 => "tember",
      10 => "ober",
      11 | 12 => "ember",
      _ => "",
    });
    Ok(month)
  }

  /// Parse a weekday abbreviation (always three letters).
  fn parse_weekday_abbr(&mut self) -> ParseResult<u8> {
    let abbr = self.pop_front(3).to_lowercase();
    Ok(match abbr.as_str() {
      "sun" => 0,
      "mon" => 1,
      "tue" => 2,
      "wed" => 3,
      "thu" => 4,
      "fri" => 5,
      "sat" => 6,
      _ => self.fail(ErrorKind::Unexpected)?,
    })
  }

  fn parse_weekday(&mut self) -> ParseResult<u8> {
    let weekday = self.parse_weekday_abbr()?;
    self.trim_front_seq(match weekday {
      0 | 1 | 5 => "day",
      2 => "sday",
      3 => "nesday",
      4 => "rsday",
      6 => "urday",
      _ => "",
    });
    Ok(weekday)
  }

  fn parse_am_pm_lower(&mut self) -> ParseResult<u8> {
    let value = match self.peek() {
      Some('a') => 0,
      Some('p') => 12,
      _ => self.fail(ErrorKind::Unexpected)?,
    };
    self.pop_front(1);
    self.expect_char('m')?;
    Ok(value)
  }

  fn parse_am_pm_upper(&mut self) -> ParseResult<u8> {
    let value = match self.peek() {
      Some('A') => 0,
      Some('P') => 12,
      _ => self.fail(ErrorKind::Unexpected)?,
    };
    self.pop_front(1);
    self.expect_char('M')?;
    Ok(value)
  }

  /// Trim all or part of the given sequence, case-insensitive. Stop at the first non-match found.
  fn trim_front_seq(&mut self, seq: &'static str) {
    for seq_char in seq.to_lowercase().chars() {
      if self.expect_char(seq_char).is_ok() {
        continue;
      };
      if self.expect_char(seq_char.to_ascii_uppercase()).is_ok() {
        continue;
      }
      break;
    }
  }

  /// Assert that no input remains, and create a parse error if it does.
  fn assert_consumed(&mut self) -> ParseResult<()> {
    if self.peek().is_some() {
      self.fail(ErrorKind::InputTooLong)?;
    }
    Ok(())
  }

  /// Generate a parse error.
  fn err(&self, kind: ErrorKind) -> ParseError {
    ParseError::new(self.src, kind)
      .at_index(self.src.len() - self.chars.clone().collect::<String>().len())
  }

  fn fail<T>(&self, kind: ErrorKind) -> ParseResult<T> {
    Err(self.err(kind))
  }
}

#[derive(Debug, Default)]
struct Partials {
  century: Option<i16>,
  year_modulo: Option<i16>,
  hour_12: Option<u8>,
  pm: Option<u8>, // 0 or 12
}

impl Partials {
  /// Return the full hour.
  fn hour(&self, src: &str) -> ParseResult<Option<u8>> {
    match (self.hour_12, self.pm) {
      (Some(12), Some(pm)) => Ok(Some(12 - pm)),
      (Some(h), Some(pm)) if h != 12 => Ok(Some(h + pm)),
      (None, None) => Ok(None),
      _ => Err(ParseError::new(src, ErrorKind::Ambiguous))?,
    }
  }

  /// Return the full year.
  fn year(&self, src: &str, opts: &ParseOptions) -> ParseResult<Option<i16>> {
    match (self.century, self.year_modulo) {
      (Some(c), Some(m)) => Ok(Some(c * 100 + m)),
      (Some(_), None) => Err(ParseError::new(src, ErrorKind::Ambiguous))?,
      (None, Some(m)) => Ok(Some((opts.modulo_year_resolution)(m))),
      (None, None) => Ok(None),
    }
  }
}
