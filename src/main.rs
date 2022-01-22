use std::fs::File;
use std::path::PathBuf;
use std::time::Instant;

use clap::{Parser, Subcommand};
use fst::automaton::Str;
use fst::{Automaton, IntoStreamer, Streamer};
use levenshtein_automata::LevenshteinAutomatonBuilder;
use memmap2::Mmap;

const POSSIBLE_TYPOS: &[&str] = &["0", "1", "2"];

/// Doc comment
#[derive(Parser)]
struct Opt {
    #[clap(long)]
    fst_path: PathBuf,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Iterate over all of the FST.
    AllSimple,

    /// Iterate over the subset of words that starts with `prefix` in the FST.
    PrefixSimple {
        #[clap(long)]
        prefix: String,
    },

    /// Shows the current technic we use to iterate over the subset of words that
    /// starts with `prefix` and with a certain amount of possible typos in the FST.
    CurrentPrefixDFA {
        #[clap(long)]
        prefix: String,
        #[clap(long, possible_values = POSSIBLE_TYPOS)]
        typos: u8,
    },

    /// Uses a new technique to iterate over the subset of words that starts
    /// with `prefix` and with a certain amount of possible typos in the FST.
    BetterPrefixDFA {
        #[clap(long)]
        prefix: String,
        #[clap(long, possible_values = POSSIBLE_TYPOS)]
        typos: u8,
        #[clap(long)]
        no_swap: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let opt = Opt::try_parse()?;

    let fst_file = File::open(opt.fst_path)?;
    let fst_mmap = unsafe { Mmap::map(&fst_file)? };
    let fst = fst::Set::new(fst_mmap)?;

    let mut count = 0;
    let before = match opt.command {
        Command::AllSimple => {
            let before = Instant::now();
            let mut iter = fst.into_stream();
            while let Some(_word) = iter.next() {
                count += 1;
            }
            before
        }
        Command::PrefixSimple { prefix } => {
            let before = Instant::now();
            let builder = fst.search(Str::new(&prefix).starts_with());
            let mut iter = builder.into_stream();
            while let Some(_word) = iter.next() {
                count += 1;
            }
            before
        }
        Command::CurrentPrefixDFA { prefix, typos } => {
            let dfa_builder = LevenshteinAutomatonBuilder::new(typos, true);
            let first_char = split_first_char(&prefix).0;

            let before = Instant::now();
            let dfa = dfa_builder.build_prefix_dfa(&prefix);
            eprintln!("dfa creation took {:.02?}", before.elapsed());
            let builder = fst.search_with_state(&dfa);
            let mut iter = builder.into_stream();
            while let Some((word, state)) = iter.next() {
                let word = unsafe { std::str::from_utf8_unchecked(word) };
                let curr_first_char = split_first_char(word).0;
                if typos == 0 {
                    count += 1;
                } else if typos == 1 && curr_first_char == first_char {
                    count += 1;
                } else if typos == 2 {
                    // We consider 1 typo on the first char as 2 typos, so we either accept:
                    // - 2 typos in the tail of the words or,
                    // - 1 typo on the first char
                    if curr_first_char == first_char {
                        count += 1;
                    } else if dfa.distance(state).to_u8() < 2 {
                        count += 1;
                    }
                }
            }
            before
        }
        Command::BetterPrefixDFA { prefix, typos, no_swap } => {
            if typos == 1 {
                let dfa_builder = LevenshteinAutomatonBuilder::new(1, true);
                let first_char = split_first_char(&prefix).0;

                let before = Instant::now();
                let dfa = dfa_builder.build_prefix_dfa(&prefix);
                eprintln!("dfa creation took {:.02?}", before.elapsed());

                let starts = Str::new(first_char).starts_with();
                let builder = fst.search(starts.intersection(dfa));

                let mut iter = builder.into_stream();
                while let Some(_word) = iter.next() {
                    count += 1;
                }

                before
            } else if typos == 2 {
                if no_swap {
                    let dfa_two_typos_builder = LevenshteinAutomatonBuilder::new(2, true);
                    let (first_char, tail) = split_first_char(&prefix);

                    let before = Instant::now();
                    let any_first_char_exact_tail = AnyFirstByteStr::new(tail).starts_with();
                    let two_typos_dfa = dfa_two_typos_builder.build_prefix_dfa(&prefix);
                    eprintln!("dfa creation took {:.02?}", before.elapsed());

                    // The first char is a typo, we search the intersect between that and
                    // what the one-typo DFA can find. Since we use damereau (swap = 1 typo)
                    // we can't optimize that further and must use this damereau levenshtein DFA.
                    let starts_with_typo = Str::new(first_char).starts_with().complement();
                    let first_typo_and_tail_one_typo =
                        starts_with_typo.intersection(any_first_char_exact_tail);

                    // The first char is valid, this is a small subset, we search two typos
                    // on the tail of the word (everything but the first char) with a two typo DFA.
                    let starts_with_first_char = Str::new(first_char).starts_with();
                    let tail_two_typos = starts_with_first_char.intersection(two_typos_dfa);

                    // We want to find the union of:
                    // - 1 typo on the first char (considered 2 by us) followed by 0 typos in the tail,
                    // - 0 typo on the first char followed by 2 typos in the tail.
                    let two_typos = first_typo_and_tail_one_typo.union(tail_two_typos);

                    let builder = fst.search(two_typos);
                    let mut iter = builder.into_stream();
                    while let Some(_word) = iter.next() {
                        count += 1;
                    }

                    before
                } else {
                    let dfa_one_typo_builder = LevenshteinAutomatonBuilder::new(1, true);
                    let dfa_two_typos_builder = LevenshteinAutomatonBuilder::new(2, true);
                    let first_char = split_first_char(&prefix).0;

                    let before = Instant::now();
                    let one_typo_dfa = dfa_one_typo_builder.build_prefix_dfa(&prefix);
                    let two_typos_dfa = dfa_two_typos_builder.build_prefix_dfa(&prefix);
                    eprintln!("dfa creation took {:.02?}", before.elapsed());

                    // The first char is a typo, we search the intersect between that and
                    // what the one-typo DFA can find. Since we use damereau (swap = 1 typo)
                    // we can't optimize that further and must use this damereau levenshtein DFA.
                    let starts_with_typo = Str::new(first_char).starts_with().complement();
                    let first_typo_and_tail_one_typo = starts_with_typo.intersection(one_typo_dfa);

                    // The first char is valid, this is a small subset, we search two typos
                    // on the tail of the word (everything but the first char) with a two typo DFA.
                    let starts_with_first_char = Str::new(first_char).starts_with();
                    let tail_two_typos = starts_with_first_char.intersection(two_typos_dfa);

                    // We want to find the union of:
                    // - 1 typo on the first char (considered 2 by us) followed by 0 typos in the tail,
                    // - 0 typo on the first char followed by 2 typos in the tail.
                    let two_typos = first_typo_and_tail_one_typo.union(tail_two_typos);

                    let builder = fst.search(two_typos);
                    let mut iter = builder.into_stream();
                    while let Some(_word) = iter.next() {
                        count += 1;
                    }

                    before
                }
            } else {
                let before = Instant::now();
                let builder = fst.search(Str::new(&prefix).starts_with());

                let mut iter = builder.into_stream();
                while let Some(_word) = iter.next() {
                    count += 1;
                }

                before
            }
        }
    };

    eprintln!("Took {:.02?} to output {} values.", before.elapsed(), count);

    Ok(())
}

fn split_first_char(s: &str) -> (&str, &str) {
    let c = s.chars().next().unwrap();
    s.split_at(c.len_utf8())
}

#[derive(Clone, Debug)]
pub struct AnyFirstByteStr<'a> {
    string: &'a [u8],
}

impl<'a> AnyFirstByteStr<'a> {
    /// Constructs automaton that matches any first char followed by the given exact string.
    #[inline]
    pub fn new(string: &'a str) -> AnyFirstByteStr<'a> {
        AnyFirstByteStr { string: string.as_bytes() }
    }
}

impl<'a> Automaton for AnyFirstByteStr<'a> {
    type State = Option<usize>;

    #[inline]
    fn start(&self) -> Option<usize> {
        Some(0)
    }

    #[inline]
    fn is_match(&self, pos: &Option<usize>) -> bool {
        // As we ignore the first char we must not forget
        // that the original string to match is length + 1
        *pos == Some(self.string.len() + 1)
    }

    #[inline]
    fn can_match(&self, pos: &Option<usize>) -> bool {
        pos.is_some()
    }

    #[inline]
    fn accept(&self, pos: &Option<usize>, byte: u8) -> Option<usize> {
        // if we aren't already past the end...
        if let Some(pos) = *pos {
            // and we are checking for the first byte, that's always true...
            if pos == 0 {
                return Some(1);
            }

            // or if there is still a matching byte at the current position + 1...
            if self.string.get(pos - 1).cloned() == Some(byte) {
                // then move forward
                return Some(pos + 1);
            }
        }
        // otherwise we're either past the end or didn't match the byte
        None
    }
}
