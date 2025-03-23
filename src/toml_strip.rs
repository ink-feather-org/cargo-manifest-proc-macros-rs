//! TODO: cleanup and remove clippy allows
#![allow(clippy::too_many_lines)]
#![allow(clippy::cognitive_complexity)]
#![allow(clippy::collapsible_else_if)]
#![allow(clippy::branches_sharing_code)]

use std::sync::LazyLock;
use tracing::trace;

/// The parameter `sections_to_keep` must be sorted.
#[must_use]
fn toml_strip(toml_str: &str, sections_to_keep: &[&str]) -> String {
  let mut stripped_toml_string = String::with_capacity(toml_str.len());

  let has_valid_section_prefix = |possible_section: &str| {
    sections_to_keep
      .binary_search_by(|prefix| {
        if possible_section.starts_with(prefix) {
          core::cmp::Ordering::Equal
        } else {
          prefix.cmp(&possible_section)
        }
      })
      .is_ok()
  };

  let mut chars_iter = toml_str.chars();
  let Some(mut current_char) = chars_iter.next() else {
    return stripped_toml_string;
  };

  let mut is_in_section = false;
  let mut is_in_relevant_section = false;

  let mut section_buffer = String::with_capacity(32);
  let mut stripped_section_buffer = String::with_capacity(32);

  'outer: loop {
    // handle section headings
    if current_char == '[' {
      is_in_section = true;
      is_in_relevant_section = false;
      section_buffer.clear();

      loop {
        current_char = match chars_iter.next() {
          Some(c) => c,
          None => break,
        };
        let is_array_of_tables = current_char == '[';
        section_buffer.push(current_char);

        if current_char == ']' {
          if is_array_of_tables {
            current_char = match chars_iter.next() {
              Some(c) => c,
              None => break,
            };
            section_buffer.push(current_char);
          }

          trace!("found_section: [{}", section_buffer);
          stripped_section_buffer.clear();
          for c in section_buffer.chars() {
            if c.is_whitespace() || c == '\"' || c == '\'' {
              continue;
            }
            stripped_section_buffer.push(c);
          }
          if has_valid_section_prefix(&stripped_section_buffer) {
            is_in_relevant_section = true;
            stripped_toml_string.push('[');
            stripped_toml_string.push_str(&section_buffer);
            trace!("is_in_relevant_section: [{}", section_buffer);
          }
          break;
        }
      }
    } else if is_in_section {
      'in_section: loop {
        // we need to handle:
        // - multi line strings
        // - multi line string literals
        // - discard comments
        match current_char {
          '#' => {
            // discard comments
            loop {
              current_char = match chars_iter.next() {
                Some(c) => c,
                None => break,
              };

              if current_char == '\n' {
                if is_in_relevant_section {
                  stripped_toml_string.push(current_char);
                }
                break;
              }
            }
          },
          '\'' => {
            // could be a normal string or a multi line string
            // double peek to check if it is a multi line string
            let is_multiline = chars_iter.as_str().starts_with("''");
            if is_multiline {
              chars_iter.next();
              chars_iter.next();
              if is_in_relevant_section {
                stripped_toml_string.push_str("\'\'\'");
              }

              // loop until a string starts with '''
              loop {
                if chars_iter.as_str().starts_with("'''") {
                  chars_iter.next();
                  chars_iter.next();
                  chars_iter.next();
                  if is_in_relevant_section {
                    stripped_toml_string.push_str("'''");
                  }
                  break;
                }
                current_char = match chars_iter.next() {
                  Some(c) => c,
                  None => break,
                };
                if is_in_relevant_section {
                  stripped_toml_string.push(current_char);
                }
              }
            } else {
              // normal literal string
              // consume until the next single quote
              if is_in_relevant_section {
                stripped_toml_string.push(current_char);
              }

              loop {
                current_char = match chars_iter.next() {
                  Some(c) => c,
                  None => break,
                };

                if is_in_relevant_section {
                  stripped_toml_string.push(current_char);
                }

                if current_char == '\'' {
                  break;
                }
              }
            }
          },
          '"' => {
            // could be a normal string or a multi line string
            // double peek to check if it is a multi line string
            let is_multiline = chars_iter.as_str().starts_with("\"\"");
            if is_multiline {
              chars_iter.next();
              chars_iter.next();
              if is_in_relevant_section {
                stripped_toml_string.push_str("\"\"\"");
              }

              // loop until a string starts with """
              loop {
                if chars_iter.as_str().starts_with("\"\"\"") {
                  chars_iter.next();
                  chars_iter.next();
                  chars_iter.next();
                  if is_in_relevant_section {
                    stripped_toml_string.push_str("\"\"\"");
                  }
                  break;
                }
                current_char = match chars_iter.next() {
                  Some(c) => c,
                  None => break,
                };
                if current_char == '\\' {
                  if is_in_relevant_section {
                    stripped_toml_string.push(current_char);
                  }

                  current_char = match chars_iter.next() {
                    Some(c) => c,
                    None => break,
                  };
                  if is_in_relevant_section {
                    stripped_toml_string.push(current_char);
                  }
                } else {
                  if is_in_relevant_section {
                    stripped_toml_string.push(current_char);
                  }
                }
              }
            } else {
              // normal string
              // consume until the next double quote but handle escape characters
              if is_in_relevant_section {
                stripped_toml_string.push(current_char);
              }

              loop {
                current_char = match chars_iter.next() {
                  Some(c) => c,
                  None => break,
                };

                if is_in_relevant_section {
                  stripped_toml_string.push(current_char);
                }

                if current_char == '"' {
                  break;
                }

                if current_char == '\\' {
                  if is_in_relevant_section {
                    stripped_toml_string.push(current_char);
                  }

                  current_char = match chars_iter.next() {
                    Some(c) => c,
                    None => break,
                  };
                  if is_in_relevant_section {
                    stripped_toml_string.push(current_char);
                  }
                }
              }
            }
          },
          '\n' => {
            // consume whitespace '\t', ' ', until the next character
            if is_in_relevant_section {
              stripped_toml_string.push(current_char);
            }

            loop {
              current_char = match chars_iter.next() {
                Some(c) => c,
                None => break,
              };

              if current_char != ' ' && current_char != '\t' {
                if current_char == '[' {
                  continue 'outer;
                }
                continue 'in_section;
              }
            }
          },
          _ => {
            if is_in_relevant_section {
              stripped_toml_string.push(current_char);
            }
          },
        }

        current_char = match chars_iter.next() {
          Some(c) => c,
          None => break,
        };
      }
    } else if current_char == '#' {
      // discard comments outside of sections
      loop {
        current_char = match chars_iter.next() {
          Some(c) => c,
          None => break,
        };

        if current_char == '\n' {
          break;
        }
      }
    } else {
      stripped_toml_string.push(current_char);
    }
    current_char = match chars_iter.next() {
      Some(c) => c,
      None => break,
    };
  }

  stripped_toml_string
}

#[must_use]
pub fn strip_irrelevant_sections_from_cargo_manifest(full_cargo_manifest_str: &str) -> String {
  static SECTIONS_TO_KEEP: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut sections_to_keep = vec![
      "workspace.dependencies",
      "workspace.dev-dependencies",
      "workspace.build-dependencies",
      "dependencies",
      "dev-dependencies",
      "build-dependencies",
      "package]",
      "workspace]",
    ];

    sections_to_keep.sort_unstable();

    sections_to_keep
  });

  toml_strip(full_cargo_manifest_str, &SECTIONS_TO_KEEP)
}

#[cfg(test)]
mod tests {
  use super::*;
  use tracing_test::traced_test;

  #[test]
  #[traced_test]
  fn test_strip_simple() {
    let full_cargo_manifest_str = r#"
# starting comment
[workspace]
members = [
    "a",
    "b",
    "c",
]

[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"

[useless_section]

[workspace.dependencies] # "
multi_line_string = """
[useless_section]
""\"""\"""\"""\"""\"
"""
multi_line_string_literal = '''
[useless_section]
'''
serde = { version = "1.0" } # comment in section
tokio = { version = "1" }
    "#;

    let expected_stripped_cargo_manifest_str = r#"
[workspace]
members = [
"a",
"b",
"c",
]

[package]
name = "test-crate"
version = "0.1.0"
edition = "2021"

[workspace.dependencies]
multi_line_string = """
[useless_section]
""\"""\"""\"""\"""\"
"""
multi_line_string_literal = '''
[useless_section]
'''
serde = { version = "1.0" }
tokio = { version = "1" }
    "#;

    let stripped_cargo_manifest_str =
      strip_irrelevant_sections_from_cargo_manifest(full_cargo_manifest_str);

    // There may be a difference in the number of empty lines so we strip them.
    fn strip_empty_lines(s: &str) -> String {
      s.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    println!("stripped_cargo_manifest_str:\n{stripped_cargo_manifest_str}");

    pretty_assertions::assert_eq!(
      strip_empty_lines(&stripped_cargo_manifest_str),
      strip_empty_lines(expected_stripped_cargo_manifest_str),
    );
  }

  #[test]
  #[traced_test]
  fn test_strip_cargo_manifest() {
    let full_cargo_manifest_str = include_str!("../tests/test_data/a_big_cargo.toml");

    let expected_stripped_cargo_manifest_str =
      include_str!("../tests/test_data/a_big_cargo_stripped.toml");

    let stripped_cargo_manifest_str =
      strip_irrelevant_sections_from_cargo_manifest(full_cargo_manifest_str);

    // There may be a difference in the number of empty lines so we strip them.
    fn strip_empty_lines(s: &str) -> String {
      s.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
    }

    println!("stripped_cargo_manifest_str:\n{stripped_cargo_manifest_str}");

    pretty_assertions::assert_eq!(
      strip_empty_lines(&stripped_cargo_manifest_str),
      strip_empty_lines(expected_stripped_cargo_manifest_str),
    );
  }
}

#[cfg(all(feature = "nightly", test))]
mod benches {
  use super::*;
  use core::hint::black_box;
  extern crate test;
  use test::Bencher;
  use toml_edit::ImDocument;

  #[bench]
  fn strip_and_parse(b: &mut Bencher) {
    let full_cargo_manifest_str = include_str!("../tests/test_data/a_big_cargo.toml");

    b.iter(|| {
      let stripped_cargo_manifest_str =
        strip_irrelevant_sections_from_cargo_manifest(full_cargo_manifest_str);
      let _ = black_box(stripped_cargo_manifest_str.parse::<ImDocument<String>>());
    });
  }

  #[bench]
  fn parse_only(b: &mut Bencher) {
    let full_cargo_manifest_str = include_str!("../tests/test_data/a_big_cargo.toml");

    b.iter(|| {
      let _ = black_box(full_cargo_manifest_str.parse::<ImDocument<String>>());
    });
  }
}
