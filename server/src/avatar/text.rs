//! One line of plain text (OWNER: avatar slice).
//!
//! [`plain`] is the one function for every piece of text an avatar carries: its name, a line, a
//! question, a choice, the content of an SVG `text` element, a fragment of a voice name. What
//! comes out is one line that shows what it holds: no line breaks, no control or format
//! characters (so nothing that reorders what follows), no stacks of combining marks and no mark
//! without a character to carry it, the session's secrets masked, and a bounded length.
//!
//! A few characters that a font draws as nothing are letters or marks by their category (the
//! Hangul fillers, the combining grapheme joiner, variation selectors) and stay, as the two
//! joiners do between letters: they are part of how some scripts and emoji are written. The mask
//! is shown the text without them too, so none of them hides a secret, and a text made of them
//! alone comes out empty, so a caller that needs text need only ask whether it got any.
//!
//! The masking step is a closure (`Terminals::redact_text` for a session), so this module needs
//! nothing from another slice and stays a pure function.
//!
//! Unicode classes come from the `regex` crate (1.13, regex-syntax 0.8, tables of Unicode 16.0):
//! the general categories `\p{Cc}`, `\p{Cf}`, `\p{Co}`, `\p{Cn}` and `\p{M}` by their short names
//! need its `unicode-gencat` feature, which is part of the default features this crate uses.
//! `\p{Cn}` is everything those tables do not assign, so a character of a newer Unicode version
//! is removed like any other unassigned one. Class difference (`[a--b]`) is supported, look-around
//! is not, so the rule for the two joiners is a scan. White space and letters are the standard
//! library's `char::is_whitespace` (the White_Space property) and `char::is_alphabetic`. The
//! boolean property `\p{Default_Ignorable_Code_Point}` needs the crate's `unicode-bool` feature,
//! also one of the defaults.

use std::sync::LazyLock;

use regex::Regex;

/// How many combining marks may follow one another.
pub const MAX_MARKS: usize = 2;

/// U+200C and U+200D: they shape Persian and Indic words, so they stay between letters.
const JOINERS: [char; 2] = ['\u{200C}', '\u{200D}'];

/// Control, format, private-use and unassigned characters, the two joiners excepted.
static GONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{Cc}\p{Cf}\p{Co}\p{Cn}--[\u{200C}\u{200D}]]+").unwrap());

/// One combining mark.
static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\p{M}$").unwrap());

/// What may stay in a text and does not show: the two joiners, and the characters a font is told
/// to draw as nothing that are marks or letters by their category (the combining grapheme joiner,
/// the Hangul fillers, variation selectors). The mask sees the text without them as well.
static UNSEEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\p{Default_Ignorable_Code_Point}+").unwrap());

fn is_mark(c: char) -> bool {
    // No ASCII character is a mark: the common case needs no table.
    !c.is_ascii() && MARK.is_match(c.encode_utf8(&mut [0; 4]))
}

/// `s` as one line of plain text of at most `max` characters.
///
/// In this order: every white-space character (line breaks, tabs, VT, FF, NEL, U+2028, U+2029
/// and the Unicode spaces) becomes a space; every control, format, private-use and unassigned
/// character is removed, except U+200C and U+200D between two letters; at most [`MAX_MARKS`]
/// combining marks stay in a row, and none at the start, where there is nothing to carry them;
/// runs of spaces collapse and the ends are trimmed; a text that shows nothing after that is
/// empty; `mask` replaces the secrets it knows; then the length is checked.
///
/// A joiner does not end a row of marks (it is invisible, so marks on both sides of it stack),
/// and of several joiners in a row one stays at most.
///
/// What stays and is invisible (a joiner, a variation selector, a filler) would let a secret
/// split by it pass the mask: the text is therefore also masked with those taken out, and when
/// that finds something they are given up. So `mask` may be called twice for one text, and has
/// to answer from its argument alone.
///
/// The error is the refusal, written to follow the name of the thing: "412 characters: at most
/// 400". An empty result is not an error; a caller that needs text checks for it.
pub fn plain(s: &str, max: usize, mask: impl Fn(&str) -> String) -> Result<String, String> {
    let spaced: String = s.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect();
    let out = tidy(&GONE.replace_all(&spaced, ""));
    // What shows nothing holds nothing: a text of fillers and joiners alone is empty.
    if !out.chars().any(|c| c != ' ' && !UNSEEN.is_match(c.encode_utf8(&mut [0; 4]))) {
        return Ok(String::new());
    }

    let masked = if UNSEEN.is_match(&out) {
        // Without what does not show, the text is tidied once more: marks and spaces may meet.
        let bare = tidy(&UNSEEN.replace_all(&out, ""));
        let masked_bare = mask(&bare);
        if masked_bare != bare { masked_bare } else { mask(&out) }
    } else {
        mask(&out)
    };

    let n = masked.chars().count();
    if n > max {
        return Err(format!("{n} characters: at most {max}"));
    }
    Ok(masked)
}

/// The rules of marks, joiners and spaces, for a text that holds no line breaks and nothing of
/// the removed classes. What it returns it leaves as it is.
fn tidy(shown: &str) -> String {
    // The marks first, with the joiners still in place and looked through: what the second pass
    // then sees next to a joiner is what stays next to it.
    let mut marked = String::with_capacity(shown.len());
    let mut row = 0;
    let mut started = false;
    for c in shown.chars() {
        if JOINERS.contains(&c) {
            // Neither a mark nor the end of a row of them.
        } else if is_mark(c) {
            if !started || row >= MAX_MARKS {
                continue;
            }
            row += 1;
        } else {
            row = 0;
            started |= c != ' ';
        }
        marked.push(c);
    }

    let mut out = String::with_capacity(marked.len());
    let mut last: Option<char> = None;
    let mut chars = marked.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ' ' {
            // A leading space and a second space in a row are dropped.
            if last.is_none_or(|l| l == ' ') {
                continue;
            }
        } else if JOINERS.contains(&c) {
            while chars.peek().is_some_and(|n| JOINERS.contains(n)) {
                chars.next();
            }
            let between_letters = last.is_some_and(char::is_alphabetic) && chars.peek().is_some_and(|n| n.is_alphabetic());
            if !between_letters {
                continue;
            }
        }
        out.push(c);
        last = Some(c);
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(s: &str) -> String {
        s.to_string()
    }

    fn clean(s: &str) -> String {
        plain(s, 1000, same).unwrap()
    }

    /// A stand-in for the session's masking: one known secret.
    fn mask_token(s: &str) -> String {
        s.replace("hunter2hunter2", "••••••")
    }

    #[test]
    fn the_unicode_classes_this_module_relies_on_exist_in_the_regex_crate() {
        // Control, format, private use, unassigned.
        for c in ['\u{0007}', '\u{009F}', '\u{200B}', '\u{202E}', '\u{E000}', '\u{F8FF}', '\u{0378}', '\u{E0041}', '\u{FFFE}', '\u{10FFFF}'] {
            assert!(GONE.is_match(&c.to_string()), "U+{:04X} is removed", c as u32);
        }
        // The joiners are left to the scan; visible text is not touched.
        for c in ['\u{200C}', '\u{200D}', 'a', 'é', '中', '😀', ' ', '\u{0301}'] {
            assert!(!GONE.is_match(&c.to_string()), "U+{:04X} is not removed by the class", c as u32);
        }
        // Combining marks of the three kinds: non-spacing, spacing, enclosing.
        for c in ['\u{0301}', '\u{0335}', '\u{0903}', '\u{20DD}', '\u{FE0F}', '\u{0E34}', '\u{034F}', '\u{E0100}'] {
            assert!(is_mark(c), "U+{:04X} is a mark", c as u32);
        }
        for c in ['a', 'é', ' ', '~', '\u{200D}', '\u{00B4}', '\u{1F3FD}', '中', '\u{3164}'] {
            assert!(!is_mark(c), "U+{:04X} is no mark", c as u32);
        }
        // What stays and does not show: the joiners, and marks and letters that a font draws as nothing.
        for c in ['\u{200C}', '\u{200D}', '\u{034F}', '\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}', '\u{17B4}', '\u{17B5}', '\u{180B}', '\u{180D}', '\u{FE00}', '\u{FE0F}', '\u{E0100}', '\u{E01EF}'] {
            assert!(UNSEEN.is_match(&c.to_string()), "U+{:04X} does not show", c as u32);
            assert!(!GONE.is_match(&c.to_string()), "U+{:04X} is not removed by the class", c as u32);
        }
        for c in ['a', ' ', '\u{0301}', '\u{2800}', '😀'] {
            assert!(!UNSEEN.is_match(&c.to_string()), "U+{:04X} shows", c as u32);
        }
    }

    #[test]
    fn plain_text_passes_unchanged() {
        assert_eq!(clean("Tests pass. Shall I open the merge request?"), "Tests pass. Shall I open the merge request?");
        assert_eq!(clean("Grüße, señor: 你好 😀"), "Grüße, señor: 你好 😀");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn line_breaks_and_tabs_become_one_space() {
        assert_eq!(clean("one\r\ntwo\nthree\rfour\tfive"), "one two three four five");
        assert_eq!(clean("a\u{000B}b\u{000C}c"), "a b c");
    }

    #[test]
    fn nel_and_the_unicode_line_and_paragraph_separators_become_one_space() {
        assert_eq!(clean("a\u{0085}b\u{2028}c\u{2029}d"), "a b c d");
        assert_eq!(clean("a\u{2028}\u{2029}\u{0085}b"), "a b");
    }

    #[test]
    fn white_space_runs_collapse_and_the_ends_are_trimmed() {
        assert_eq!(clean("   a  \t \n  b   "), "a b");
        assert_eq!(clean("a\u{00A0}\u{2003}\u{3000}b"), "a b");
        assert_eq!(clean(" \t\n "), "");
        // A removed character between two spaces leaves one space.
        assert_eq!(clean("a \u{200B} b"), "a b");
    }

    #[test]
    fn control_characters_are_removed() {
        assert_eq!(clean("a\u{0000}b\u{0007}c\u{001B}[31md\u{007F}e\u{009B}f"), "abc[31mdef");
    }

    #[test]
    fn bidirectional_overrides_and_isolates_are_removed() {
        assert_eq!(clean("abc\u{202E}dcba\u{202C}"), "abcdcba");
        assert_eq!(clean("\u{202A}a\u{202B}b\u{202D}c\u{2066}d\u{2067}e\u{2068}f\u{2069}g\u{200E}h\u{200F}i\u{061C}j"), "abcdefghij");
    }

    #[test]
    fn zero_width_characters_are_removed() {
        assert_eq!(clean("a\u{200B}b\u{2060}c\u{FEFF}d\u{00AD}e\u{180E}f"), "abcdef");
        // The joiners outside letters go as well.
        assert_eq!(clean("\u{200D}a 1\u{200C}2 b\u{200D}"), "a 12 b");
        assert_eq!(clean("a\u{200D} b"), "a b");
        assert_eq!(clean("😀\u{200D}😀"), "😀😀");
    }

    #[test]
    fn the_joiners_stay_between_letters() {
        // Persian "mi-khaham": the non-joiner keeps the two parts apart.
        assert_eq!(clean("می\u{200C}خواهم"), "می\u{200C}خواهم");
        assert_eq!(clean("a\u{200D}b"), "a\u{200D}b");
        // A run of joiners between letters leaves one.
        assert_eq!(clean("a\u{200D}\u{200C}\u{200D}b"), "a\u{200D}b");
    }

    #[test]
    fn tag_characters_private_use_and_unassigned_characters_are_removed() {
        assert_eq!(clean("a\u{E0041}\u{E0042}\u{E007F}b"), "ab");
        assert_eq!(clean("a\u{E000}b\u{F0000}c\u{10FFFD}d"), "abcd");
        assert_eq!(clean("a\u{0378}b\u{FFFF}c"), "abc");
    }

    #[test]
    fn at_most_two_combining_marks_stay_in_a_row() {
        assert_eq!(clean("e\u{0301}"), "e\u{0301}");
        assert_eq!(clean("e\u{0301}\u{0302}"), "e\u{0301}\u{0302}");
        assert_eq!(clean("e\u{0301}\u{0302}\u{0303}\u{0304}\u{0305}x"), "e\u{0301}\u{0302}x");
        // Marks that only touch once a hidden character between them is gone count as one run.
        assert_eq!(clean("e\u{0301}\u{200B}\u{0302}\u{200B}\u{0303}"), "e\u{0301}\u{0302}");
        let zalgo: String = std::iter::once('Z').chain(std::iter::repeat_n('\u{0335}', 500)).collect();
        assert_eq!(clean(&zalgo).chars().count(), 3);
    }

    #[test]
    fn a_secret_split_by_a_zero_width_space_is_masked() {
        let split = "the password is hunter2\u{200B}hunter2 today";
        assert_eq!(plain(split, 100, mask_token).unwrap(), "the password is •••••• today");
        let split = "hun\u{200B}ter2\u{2060}hun\u{FEFF}ter2";
        assert_eq!(plain(split, 100, mask_token).unwrap(), "••••••");
    }

    #[test]
    fn a_secret_split_by_a_joiner_between_letters_is_masked() {
        // The joiner would stay (it sits between letters), so the mask also sees the text without it.
        let split = "say hunter2hun\u{200D}ter2 now";
        assert_eq!(plain(split, 100, mask_token).unwrap(), "say •••••• now");
        // Without a secret the joiner stays.
        assert_eq!(plain("a\u{200D}b", 100, mask_token).unwrap(), "a\u{200D}b");
    }

    #[test]
    fn the_length_is_checked_after_cleaning_and_masking_and_refuses() {
        assert_eq!(plain("abcde", 5, same).unwrap(), "abcde");
        assert_eq!(plain("abcdef", 5, same).unwrap_err(), "6 characters: at most 5");
        // Characters, not bytes.
        assert_eq!(plain("ééééé", 5, same).unwrap(), "ééééé");
        // What is removed does not count.
        assert_eq!(plain("  a\u{200B}b\u{202E}c  \n", 3, same).unwrap(), "abc");
        // The masked text is what is measured.
        assert_eq!(plain("hunter2hunter2", 6, mask_token).unwrap(), "••••••");
        assert!(plain("hunter2hunter2 and more", 6, mask_token).is_err());
    }

    #[test]
    fn hostile_input_of_any_length_is_handled() {
        let long: String = "a\u{200D}".repeat(50_000);
        assert!(plain(&long, 10, same).is_err());
        // Marks with nothing to carry them leave nothing; behind a letter, the two that may stay.
        let marks: String = "\u{0301}".repeat(100_000);
        assert_eq!(clean(&marks), "");
        assert_eq!(clean(&format!("a{marks}")).chars().count(), 3);
        let hidden: String = "\u{202E}".repeat(100_000);
        assert_eq!(clean(&hidden), "");
    }
    // ------------------------------------------------------------ the adversary

    /// A stand-in for the session's masking that also tells what it was shown.
    fn seen_by_the_mask(s: &str) -> Vec<String> {
        let seen = std::cell::RefCell::new(Vec::new());
        let _ = plain(s, 1000, |text| {
            seen.borrow_mut().push(text.to_string());
            text.to_string()
        });
        seen.into_inner()
    }

    #[test]
    fn marks_do_not_stack_through_joiners() {
        // A joiner between two marks is not between letters: it goes, and the marks on both sides of it are one row.
        assert_eq!(clean("e\u{0301}\u{0302}\u{200D}\u{0303}\u{0304}\u{200C}\u{0305}\u{0306}x"), "e\u{0301}\u{0302}x");
        assert_eq!(clean("e\u{0301}\u{200D}\u{0302}\u{200D}\u{0303}"), "e\u{0301}\u{0302}");
        let zalgo: String = std::iter::once('Z').chain("\u{0335}\u{200D}".chars().cycle().take(1000)).collect();
        assert_eq!(clean(&zalgo), "Z\u{0335}\u{0335}");
        // Marks that count as letters (a Thai vowel sign, a Devanagari one) keep a joiner between them, and still do not stack.
        for mark in ['\u{0E34}', '\u{0941}', '\u{0650}'] {
            assert!(mark.is_alphabetic() && is_mark(mark));
            let stack: String = std::iter::once('\u{0E01}').chain([mark, '\u{200D}'].into_iter().cycle().take(400)).collect();
            let cleaned = clean(&stack);
            assert_eq!(cleaned.chars().filter(|c| *c == mark).count(), MAX_MARKS, "U+{:04X}", mark as u32);
            assert!(cleaned.chars().count() <= 2 + MAX_MARKS, "{cleaned:?}");
        }
        // Different marks, enclosing ones, variation selectors: a row is a row.
        assert_eq!(clean("a\u{0301}\u{20DD}\u{FE0F}\u{0489}b"), "a\u{0301}\u{20DD}b");
        assert_eq!(clean("a\u{FE0F}\u{FE0F}\u{FE0F}\u{FE0F}"), "a\u{FE0F}\u{FE0F}");
        // A space, a letter or a sign ends a row; what does not show and is removed does not.
        assert_eq!(clean("a\u{0301}\u{0302} \u{0303}\u{0304}b\u{0305}\u{0306}-\u{0307}"), "a\u{0301}\u{0302} \u{0303}\u{0304}b\u{0305}\u{0306}-\u{0307}");
        assert_eq!(clean("a\u{0301}\u{0302}\u{200B}\u{2060}\u{FEFF}\u{202E}\u{0303}"), "a\u{0301}\u{0302}");
        // What a keyboard writes stays whole: an accent, a keycap, a heart in colour, a skin tone, a flag.
        for text in ["e\u{0301}", "1\u{FE0F}\u{20E3}", "\u{2764}\u{FE0F}", "\u{1F44D}\u{1F3FD}", "\u{1F1EC}\u{1F1E7}", "\u{0915}\u{094D}\u{0937}", "ก\u{0E34}\u{0E49}"] {
            assert_eq!(clean(text), text);
        }
    }

    #[test]
    fn a_mark_at_the_start_has_nothing_to_carry_it_and_goes() {
        assert_eq!(clean("\u{0301}abc"), "abc");
        assert_eq!(clean("\u{0301}"), "");
        assert_eq!(clean("\u{0338}\u{0338}\u{20DD}"), "");
        assert_eq!(clean(" \n\u{0301}\u{0302} a"), "a");
        assert_eq!(clean("\u{FE0F}\u{20E3}1"), "1");
        // Behind what is removed or dropped, it is still at the start.
        assert_eq!(clean("\u{202E}\u{0301}a"), "a");
        assert_eq!(clean("\u{200D}\u{0301}\u{200D}a"), "a");
        assert_eq!(clean("\u{200B} \u{0489}"), "");
        // Inside the text a mark may stand on a space: that is how an accent is shown alone.
        assert_eq!(clean("a \u{0301}b"), "a \u{0301}b");
        assert_eq!(clean("x\u{0301}"), "x\u{0301}");
        assert_eq!(clean("\u{1F600}\u{FE0F}"), "\u{1F600}\u{FE0F}");
        assert_eq!(clean("-\u{0301}"), "-\u{0301}");
    }

    #[test]
    fn joiners_stay_only_between_letters_and_one_at_a_time() {
        // Emoji are joined by U+200D into one picture; they are not letters, so the parts stand alone.
        assert_eq!(clean("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"), "\u{1F468}\u{1F469}\u{1F467}");
        assert_eq!(clean("\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}"), "\u{1F3F3}\u{FE0F}\u{1F308}");
        // A flag of tag characters is its black flag.
        assert_eq!(clean("\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}"), "\u{1F3F4}");
        // Persian and Urdu: the non-joiner inside a word.
        for word in ["می\u{200C}خواهم", "کتاب\u{200C}ها", "نیم\u{200C}فاصله"] {
            assert_eq!(clean(word), word);
        }
        // At a word's edge, next to a digit, a sign or a space, or doubled, it goes.
        assert_eq!(clean("می\u{200C} خواهم"), "می خواهم");
        assert_eq!(clean("\u{200C}می\u{200C}"), "می");
        assert_eq!(clean("a\u{200D}1 2\u{200C}b c\u{200D}-d (\u{200D})"), "a1 2b c-d ()");
        assert_eq!(clean("a\u{200D}\u{200D}\u{200D}\u{200D}b"), "a\u{200D}b");
        assert_eq!(clean("a\u{200C}\u{200D}b"), "a\u{200C}b");
        assert_eq!(clean("a\u{200D}\u{200B}\u{200D}b"), "a\u{200D}b");
        assert_eq!(clean("a\u{200D}\u{200D}"), "a");
        // A hundred thousand of them cost little.
        let many = format!("a{}b", "\u{200D}".repeat(100_000));
        let started = std::time::Instant::now();
        assert_eq!(clean(&many), "a\u{200D}b");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_secret_split_by_anything_that_does_not_show_is_masked() {
        for hidden in ["\u{034F}", "\u{3164}", "\u{115F}", "\u{1160}", "\u{FFA0}", "\u{17B4}", "\u{17B5}", "\u{180B}", "\u{180D}", "\u{FE00}", "\u{FE0F}", "\u{E0100}", "\u{200D}", "\u{200C}", "\u{034F}\u{FE0F}", "\u{200B}", "\u{2060}", "\u{00AD}", "\u{202E}", "\u{E0041}", "\u{FEFF}", "\u{0000}", "\u{001B}", "\u{E000}"] {
            let split = format!("it is hunter2{hidden}hunter2 now");
            assert_eq!(plain(&split, 100, mask_token).unwrap(), "it is •••••• now", "split by U+{:04X}", hidden.chars().next().unwrap() as u32);
            // Split in several places at once.
            let pieces: String = "hunter2hunter2".chars().flat_map(|c| [c].into_iter().chain(hidden.chars())).collect();
            assert_eq!(plain(&pieces, 100, mask_token).unwrap(), "••••••");
        }
        // What shows nothing at all is empty, whatever it is made of; next to something that shows, it stays.
        for nothing in ["\u{3164}", "\u{3164}\u{3164} \u{115F}", "\u{FFA0}\u{034F}", "\u{3164}\u{FE0F} \u{17B5}", " \u{1160}\u{200D}\u{1160} "] {
            assert_eq!(plain(nothing, 100, mask_token).unwrap(), "", "{nothing:?}");
        }
        assert_eq!(seen_by_the_mask("\u{3164} \u{3164}"), [""; 0]);
        assert_eq!(plain("\u{3164}.", 100, mask_token).unwrap(), "\u{3164}.");
        assert_eq!(plain("\u{3164}\u{0301}", 100, mask_token).unwrap(), "\u{3164}\u{0301}");
        // Without a secret what does not show and may stay, stays.
        assert_eq!(plain("\u{2764}\u{FE0F} a\u{200D}b \u{1112}\u{1161}\u{11AB}", 100, mask_token).unwrap(), "\u{2764}\u{FE0F} a\u{200D}b \u{1112}\u{1161}\u{11AB}");
        // The mask is shown the text as it will stand, and the text without what does not show when there is any.
        assert_eq!(seen_by_the_mask(" a\u{200B}b\n"), ["ab"]);
        assert_eq!(seen_by_the_mask("a\u{200D}b \u{2764}\u{FE0F}"), ["ab \u{2764}", "a\u{200D}b \u{2764}\u{FE0F}"]);
        // A secret is masked before the length is taken, and the masked text is what is measured.
        assert_eq!(plain("hunter2\u{034F}hunter2", 6, mask_token).unwrap(), "••••••");
        // What the mask is shown, and so what comes out when it finds something, is one tidy line as well.
        assert_eq!(plain("a \u{3164} hunter2\u{3164}hunter2 \u{3164}", 100, mask_token).unwrap(), "a ••••••");
        assert_eq!(plain("\u{3164}\u{0301}hunter2hunter2", 100, mask_token).unwrap(), "••••••");
        assert_eq!(plain("x\u{0301}\u{0302}\u{3164}\u{0303}\u{0304} hunter2\u{034F}hunter2", 100, mask_token).unwrap(), "x\u{0301}\u{0302} ••••••");
        assert_eq!(seen_by_the_mask("a \u{3164} b"), ["a b", "a \u{3164} b"]);
    }

    #[test]
    fn the_length_rule_counts_characters_of_what_is_left() {
        assert_eq!(plain("", 0, same).unwrap(), "");
        assert_eq!(plain(" \u{200B}\n", 0, same).unwrap(), "");
        assert_eq!(plain("a", 0, same).unwrap_err(), "1 characters: at most 0");
        // A character is a character, however many bytes or marks.
        assert_eq!(plain("😀😀😀", 3, same).unwrap(), "😀😀😀");
        assert_eq!(plain("e\u{0301}e\u{0301}", 3, same).unwrap_err(), "4 characters: at most 3");
        assert_eq!(plain("😀😀😀😀", 3, same).unwrap_err(), "4 characters: at most 3");
        // What is removed is not counted: marks past the two, spaces past the one, what does not show.
        assert_eq!(plain(&format!("a{}  \t b\u{202E}", "\u{0301}".repeat(50)), 5, same).unwrap(), "a\u{0301}\u{0301} b");
        // The refusal names the count and never the text.
        let long = "<script>".repeat(1000);
        assert_eq!(plain(&long, 400, same).unwrap_err(), "8000 characters: at most 400");
    }

    /// The same characters on every run: a sweep, not a gamble.
    fn sequence(mut seed: u64) -> impl FnMut(usize) -> usize {
        move |below: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % below.max(1) as u64) as usize
        }
    }

    #[test]
    fn whatever_goes_in_what_comes_out_is_one_line_that_shows_what_it_holds() {
        let pool = [
            'a', 'b', 'Z', '1', '-', ' ', ' ', '\t', '\n', '\r', '\u{000B}', '\u{000C}', '\u{0085}', '\u{00A0}', '\u{2028}', '\u{2029}', '\u{3000}', '\u{0000}', '\u{001B}', '\u{007F}', '\u{009B}', '\u{00AD}',
            '\u{200B}', '\u{200C}', '\u{200D}', '\u{200E}', '\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{2060}', '\u{FEFF}', '\u{061C}', '\u{E000}', '\u{E0001}', '\u{E0041}', '\u{FFFE}', '\u{0378}', '\u{10FFFF}',
            '\u{0301}', '\u{0335}', '\u{20DD}', '\u{FE0F}', '\u{034F}', '\u{0E34}', '\u{0941}', '\u{094D}', '\u{3164}', '\u{115F}', 'é', 'ی', 'م', 'क', 'ก', '中', '😀', '\u{1F468}', '\u{2764}', '\u{1F3FD}', '\u{1F1EC}',
        ];
        let mut pick = sequence(0x9E37_79B9_7F4A_7C15);
        for _ in 0..4000 {
            let text: String = (0..pick(24)).map(|_| pool[pick(pool.len())]).collect();
            let out = clean(&text);
            let chars: Vec<char> = out.chars().collect();
            // One line, nothing that does not show or reorders, the two joiners excepted.
            assert!(!chars.iter().any(|c| c.is_control() || (GONE.is_match(&c.to_string()))), "{text:?} -> {out:?}");
            assert!(!chars.iter().any(|c| c.is_whitespace() && *c != ' '), "{text:?} -> {out:?}");
            assert!(!out.starts_with(' ') && !out.ends_with(' ') && !out.contains("  "), "{text:?} -> {out:?}");
            // A joiner stands alone between two letters.
            for (i, c) in chars.iter().enumerate() {
                if JOINERS.contains(c) {
                    assert!(i > 0 && i + 1 < chars.len() && chars[i - 1].is_alphabetic() && chars[i + 1].is_alphabetic(), "{text:?} -> {out:?}");
                }
            }
            // What is not empty shows something.
            assert!(chars.is_empty() || chars.iter().any(|c| *c != ' ' && !UNSEEN.is_match(&c.to_string())), "{text:?} -> {out:?}");
            // No mark at the start, and no more than two in a row, joiners looked through.
            assert!(!chars.first().is_some_and(|c| is_mark(*c)), "{text:?} -> {out:?}");
            let mut row = 0;
            for c in &chars {
                if is_mark(*c) {
                    row += 1;
                    assert!(row <= MAX_MARKS, "{text:?} -> {out:?}");
                } else if !JOINERS.contains(c) {
                    row = 0;
                }
            }
            // Cleaning what is clean changes nothing, and never makes it longer.
            assert_eq!(clean(&out), out, "{text:?}");
            assert!(chars.len() <= text.chars().count());
            // The length rule holds to the character.
            assert_eq!(plain(&text, chars.len(), same).as_deref(), Ok(out.as_str()));
            if !chars.is_empty() {
                assert_eq!(plain(&text, chars.len() - 1, same).unwrap_err(), format!("{} characters: at most {}", chars.len(), chars.len() - 1));
            }
        }
    }
}
