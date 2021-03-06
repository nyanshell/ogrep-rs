extern crate atty;
#[macro_use] extern crate clap;
extern crate ansi_term;
extern crate regex;
extern crate itertools;

use std::ffi::OsStr;
use std::path::PathBuf;
use std::borrow::Cow;
use regex::{Regex, RegexBuilder};

use std::io::BufRead;
use std::io::Write as IoWrite;
use std::os::unix::ffi::OsStrExt;
use std::fmt::Write as FmtWrite;

// This prefixes are used when "smart branches" feature
// is turned on. When line starts with given prefix, then retain
// lines with same indentation starting with given prefixes in context.
const SMART_BRANCH_PREFIXES: &[(&str, &[&str])] = &[
    ("} else ", &["if ", "} else if "]),
    ("case ", &["switch "]),
];

enum InputSpec {
    File(PathBuf),
    Stdin,
}

enum Input {
    File(std::io::BufReader<std::fs::File>),
    Stdin(std::io::Stdin),
}
enum InputLock<'a> {
    File(&'a mut std::io::BufReader<std::fs::File>),
    Stdin(std::io::StdinLock<'a>),
}
impl Input {
    fn open(spec: &InputSpec) -> std::io::Result<Self> {
        match *spec {
            InputSpec::File(ref path) => {
                let file = std::fs::File::open(path)?;
                Ok(Input::File(std::io::BufReader::new(file)))
            },
            InputSpec::Stdin => Ok(Input::Stdin(std::io::stdin())),
        }
    }
    fn lock(&mut self) -> InputLock {
        match *self {
            Input::File(ref mut file) => InputLock::File(file),
            Input::Stdin(ref mut stdin) => InputLock::Stdin(stdin.lock()),
        }
    }
}
impl<'a> InputLock<'a> {
    fn as_buf_read(&mut self) -> &mut std::io::BufRead {
        match self {
            &mut InputLock::File(ref mut reader) => reader,
            &mut InputLock::Stdin(ref mut lock) => lock,
        }
    }
}

arg_enum!{
    #[derive(Debug)]
    pub enum UseColors { Always, Auto, Never }
}

arg_enum!{
    #[derive(Debug)]
    pub enum Preprocessor { Context, Ignore, Preserve }
}

struct Options {
    pattern: String,
    input: InputSpec,
    regex: bool,
    case_insensitive: bool,
    whole_word: bool,
    use_colors: UseColors,
    breaks: bool,
    ellipsis: bool,
    print_filename: bool,
    smart_branches: bool,
    preprocessor: Preprocessor,
}

struct AppearanceOptions {
    use_colors: bool,
    breaks: bool,
    ellipsis: bool,
    print_filename: bool,
}

struct Printer {
    options: AppearanceOptions,
}
impl Printer {
    fn print_context(&self, line_number: usize, line: &str) {
        if self.options.use_colors {
            let text = format!("{:4}: {}", line_number, line);
            println!("{}", ansi_term::Style::new().dimmed().paint(text));
        } else {
            println!("{:4}: {}", line_number, line);
        }
    }

    fn print_match<'m, M>(&self, line_number: usize, line: &str, matches: M)
            where M: Iterator<Item=regex::Match<'m>> {
        if self.options.use_colors {
            let match_style = ansi_term::Style::new().bold();

            let mut buf = String::new();
            let mut pos = 0usize;
            for m in matches {
                buf.push_str(&line[pos..m.start()]);
                write!(&mut buf, "{}", match_style.paint(m.as_str())).unwrap();
                pos = m.end();
            }
            buf.push_str(&line[pos..]);

            println!("{:4}: {}", line_number, buf);

        } else {
            println!("{:4}: {}", line_number, line);
        }
    }

    fn print_break(&self) {
        if self.options.breaks {
            println!();
        }
    }

    fn print_ellipsis(&self) {
        if self.options.ellipsis {
            println!("   {}", ansi_term::Style::new().dimmed().paint("…"));
        }
    }

    fn print_filename(&self, filename: &std::path::Path) {
        if self.options.print_filename {
            let mut stdout = std::io::stdout();
            stdout.write(b"\n").unwrap();
            if self.options.use_colors {
                let style = ansi_term::Style::new().underline();
                style.paint(filename.as_os_str().as_bytes()).write_to(&mut stdout).unwrap();
            } else {
                stdout.write_all(filename.as_os_str().as_bytes()).unwrap();
            }
            stdout.write(b"\n\n").unwrap();
        }
    }
}

fn parse_arguments() -> Options {
    use clap::{App, Arg};

    let colors_default = UseColors::Auto.to_string();
    let preprocessor_default = Preprocessor::Context.to_string();

    let matches = App::new(crate_name!())
        .about(crate_description!())
        .author(crate_authors!("\n"))
		.version(crate_version!())
        .arg(Arg::with_name("pattern")
            .help("Pattern to search for")
            .required(true))
        .arg(Arg::with_name("input")
            .help("File to search in"))
        .arg(Arg::with_name("regex")
            .short("e")
            .long("regex")
            .help("Treat pattern as regular expression"))
        .arg(Arg::with_name("case-insensitive")
            .short("i")
            .long("case-insensitive")
            .help("Perform case-insensitive matching"))
        .arg(Arg::with_name("whole-word")
            .short("w")
            .long("word")
            .help("Search for whole words matching pattern"))
        .arg(Arg::with_name("color")
            .long("color")
            .takes_value(true)
            .default_value(&colors_default)
            .possible_values(&UseColors::variants())
            .case_insensitive(true)
            .help("File to search in"))
        .arg(Arg::with_name("no-breaks")
            .long("no-breaks")
            .help("Don't preserve line breaks"))
        .arg(Arg::with_name("ellipsis")
            .long("ellipsis")
            .help("Print ellipsis when lines were skipped"))
        .arg(Arg::with_name("print-filename")
            .long("print-filename")
            .help("Print filename on match"))
        .arg(Arg::with_name("no-smart-branches")
            .long("no-smart-branches")
            .help("Don't handle if/if-else/else conditionals specially"))
        .arg(Arg::with_name("preprocessor")
            .long("preprocessor")
            .takes_value(true)
            .default_value(&preprocessor_default)
            .possible_values(&Preprocessor::variants())
            .case_insensitive(true)
            .help("How to handle C preprocessor instructions"))
        .get_matches();

    Options {
        pattern: matches.value_of("pattern").expect("pattern").to_string(),
        input: match matches.value_of_os("input").unwrap_or(OsStr::new("-")) {
          path if path == "-" => InputSpec::Stdin,
          path => InputSpec::File(PathBuf::from(path)),
        },
        regex: matches.is_present("regex"),
        case_insensitive: matches.is_present("case-insensitive"),
        whole_word: matches.is_present("whole-word"),
        use_colors: value_t!(matches, "color", UseColors).unwrap_or_else(|e| e.exit()),
        breaks: !matches.is_present("no-breaks"),
        ellipsis: matches.is_present("ellipsis"),
        print_filename: matches.is_present("print-filename"),
        smart_branches: !matches.is_present("no-smart-branches"),
        preprocessor: value_t!(matches, "preprocessor", Preprocessor).unwrap_or_else(|e| e.exit()),
    }
}

fn calculate_indentation(s: &str) -> Option<usize> {
    s.find(|c: char| !c.is_whitespace())
}

struct ContextEntry {
    line_number: usize,
    indentation: usize,
    line: String,
}

enum PreprocessorKind { If, Else, Endif, Other }
fn preprocessor_instruction_kind(s: &str) -> Option<PreprocessorKind> {
    match s {
        _ if s.starts_with("#if ") => Some(PreprocessorKind::If),
        _ if s.starts_with("#else") => Some(PreprocessorKind::Else),
        _ if s.starts_with("#endif") => Some(PreprocessorKind::Endif),
        _ if s.starts_with("#") => Some(PreprocessorKind::Other),
        _ => None,
    }
}

fn process_input(input: &mut BufRead, pattern: &Regex, options: &Options, printer: &Printer) -> std::io::Result<()> {
    // Context of current line. Last context item contains closest line above current
    // whose indentation is lower than one of a current line. One before last
    // item contains closest line above last context line with lower indentation and
    // so on. Once line is printed, it is removed from context.
    // Context may contain lines with identical identation due to smart if-else branches
    // handling.
    let mut context = Vec::new();

    // Secondary stack for preprocessor instructions.
    let mut preprocessor_level = 0usize;
    let mut preprocessor_context = Vec::new();

    // Whether at least one match was already found.
    let mut match_found = false;

    // Whether empty line was met since last match.
    let mut was_empty_line = false;

    let mut last_printed_lineno = 0usize;

    for (line_number, line) in input.lines().enumerate().map(|(n, l)| (n+1, l)) {
        let line = line?;
        let indentation = match calculate_indentation(&line) {
            Some(ind) => ind,
            None => {
                was_empty_line = true;
                continue;
            }
        };

        // Ignore lines looking like C preprocessor instruction, because they
        // are often written without indentation and this breaks context.
        match options.preprocessor {
            Preprocessor::Preserve => (), // Do nothing, handle line as usual
            Preprocessor::Ignore =>
                if preprocessor_instruction_kind(&line[indentation..]).is_some() {
                    continue;
                },
            Preprocessor::Context =>
                match preprocessor_instruction_kind(&line[indentation..]) {
                    None => (),
                    Some(PreprocessorKind::If) => {
                        preprocessor_level += 1;
                        preprocessor_context.push(ContextEntry { line_number, indentation: preprocessor_level, line });
                        continue;
                    },
                    Some(PreprocessorKind::Else) => {
                        preprocessor_context.push(ContextEntry { line_number, indentation: preprocessor_level, line });
                        continue;
                    },
                    Some(PreprocessorKind::Endif) => {
                        let top = preprocessor_context.iter().rposition(|e: &ContextEntry| {
                            e.indentation < preprocessor_level
                        });
                        preprocessor_context.truncate(top.map(|t| t+1).unwrap_or(0));
                        preprocessor_level -= 1;
                        continue;
                    },
                    Some(PreprocessorKind::Other) => continue,
                },
        }

        let top = context.iter().rposition(|e: &ContextEntry| {
            // Upper scopes are always preserved.
            if e.indentation < indentation { return true; }
            if e.indentation > indentation { return false; }

            if !options.smart_branches { return false; }

            let stripped_line = &line[indentation..];
            let stripped_context_line = &e.line[e.indentation..];
            for &(prefix, context_prefixes) in SMART_BRANCH_PREFIXES {
                if stripped_line.starts_with(prefix) {
                    return context_prefixes.iter().any(|p| stripped_context_line.starts_with(p));
                }
            }

            return false;
        });
        context.truncate(top.map(|t| t+1).unwrap_or(0));

        let matched = {
            let mut matches = pattern.find_iter(&line).peekable();
            if matches.peek().is_some() {
                // `match_found` is checked to avoid extra line break before first match.
                if !match_found {
                    if let InputSpec::File(ref path) = options.input {
                        printer.print_filename(path)
                    }
                }
                if was_empty_line && match_found {
                    printer.print_break();
                }

                {
                    let combined_context = itertools::merge_join_by(&context, &preprocessor_context, |ci, pci|
                        ci.line_number.cmp(&pci.line_number)
                    ).map(|either| {
                        use itertools::EitherOrBoth::{Left, Right, Both};
                        match either {
                            Left(l) => l,
                            Right(l) => l,
                            Both(_, _) => unreachable!(),
                        }
                    });

                    for entry in combined_context {
                        if (!was_empty_line || !printer.options.breaks) &&
                           entry.line_number > last_printed_lineno + 1 {
                            printer.print_ellipsis();
                        }
                        printer.print_context(entry.line_number, &entry.line);
                        last_printed_lineno = entry.line_number;
                    }

                    if (!was_empty_line || !printer.options.breaks) &&
                       line_number > last_printed_lineno + 1 {
                        printer.print_ellipsis();
                    }
                    printer.print_match(line_number, &line, matches);
                    last_printed_lineno = line_number;
                }

                context.clear();
                preprocessor_context.clear();
                was_empty_line = false;
                match_found = true;

                true
            } else {
                false
            }
        };

        if !matched {
            context.push(ContextEntry { line_number, indentation, line });
        }
    }

    Ok(())
}

fn real_main() -> std::result::Result<i32, Box<std::error::Error>> {
    let options = parse_arguments();

    let appearance = AppearanceOptions {
        use_colors: match options.use_colors {
            UseColors::Always => true,
            UseColors::Never => false,
            UseColors::Auto => atty::is(atty::Stream::Stdout),
        },
        breaks: options.breaks,
        ellipsis: options.ellipsis,
        print_filename: options.print_filename,
    };

    let printer = Printer { options: appearance };

    let mut pattern: Cow<str> = if options.regex { Cow::from(options.pattern.as_ref()) }
                                else { Cow::from(regex::escape(&options.pattern)) };
    if options.whole_word {
        let p = pattern.to_mut();
        p.insert_str(0, r"\b");
        p.push_str(r"\b");
    }
    let re = RegexBuilder::new(&pattern).case_insensitive(options.case_insensitive).build()?;

    let mut input = Input::open(&options.input)?;
    let mut input_lock = input.lock();
    process_input(input_lock.as_buf_read(), &re, &options, &printer)?;
    Ok(0)
}

fn main() {
    match real_main() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            writeln!(std::io::stderr(), "{}", err.description()).unwrap();
            std::process::exit(127);
        },
    }
}

