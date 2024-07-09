use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use md_regex_parser::{MDLinkParser, MDRegexParseable};
use regex::Captures;
use vault::Vault;

use crate::Location;

pub(crate) struct Parser<'a> {
    memfs: &'a dyn ParserMemfs,
}

impl<'a> Parser<'a> {
    pub fn parse_entity_query(
        &self,
        location: Location<'a>,
    ) -> Option<(NamedRefCmdQuery, QueryMetadata)> {
        self.parse_query(location)
    }

    pub fn parse_block_query(
        &self,
        location: Location<'a>,
    ) -> Option<(BlockLinkCmdQuery, QueryMetadata)> {
        self.parse_query(location)
    }

    fn line_string(&self, location: Location) -> Option<&'a str> {
        self.memfs.select_line_str(location.path, location.line)
    }

    fn parse_query<T: MDRegexParseable<'a>>(&self, location: Location<'a>) -> ParseQueryResult<T> {
        let line_string = self.line_string(location)?;
        let (q, char_range, info) = {
            let character = location.character;
            MDLinkParser::new(line_string, character as usize).parse()
        }?;

        let new_query_metadata = QueryMetadata::new(location, char_range, info);

        Some((q, new_query_metadata))
    }
}

type ParseQueryResult<'a, T: MDRegexParseable<'a>> = Option<(T, QueryMetadata)>;

// NOTE: Enables mocking for tests and provides a slight benefit of decoupling Parser from vault as
// memfs -- which will eventually be replaced by a true MemFS crate.
trait ParserMemfs: Send + Sync {
    fn select_line_str(&self, path: &Path, line: u32) -> Option<&str>;
}

impl ParserMemfs for Vault {
    fn select_line_str(&self, path: &Path, line: u32) -> Option<&str> {
        self.select_line_str(path, line as usize)
    }
}

impl<'a> Parser<'a> {
    pub(crate) fn new(vault: &'a Vault) -> Self {
        Self {
            memfs: vault as &dyn ParserMemfs,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct NamedRefCmdQuery<'a> {
    pub file_query: &'a str,
    pub infile_query: Option<EntityInfileQuery<'a>>,
}

impl NamedRefCmdQuery<'_> {
    // NOTE: this is sort or re-implemented by multiple traits with methods meaning the same thing, but centralizing the implementation here
    // prevents duplication
    pub fn grep_string(&self) -> String {
        match &self.infile_query {
            Some(EntityInfileQuery::Heading(h)) => format!("{}#{}", self.file_query, h),
            Some(EntityInfileQuery::Index(i)) => format!("{}#^{}", self.file_query, i),
            None => self.file_query.to_string(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum EntityInfileQuery<'a> {
    /// Can be empty excludes the #
    Heading(&'a str),
    /// Can be empty; excludes the ^
    Index(&'a str),
}

#[derive(Debug, PartialEq)]
/// DATA
pub struct BlockLinkCmdQuery {
    grep_string: String,
}

impl BlockLinkCmdQuery {
    pub fn grep_string(&self) -> String {
        self.grep_string
            .to_string()
            .replace(r"\[", "[")
            .replace(r"\]", "]")
    }
    pub fn display_grep_string(&self) -> String {
        self.grep_string
            .to_string()
            .replace(r"\[", "")
            .replace(r"\]", "")
    }
}

#[derive(Debug, Clone)]
pub struct QueryMetadata {
    pub line: u32,
    pub char_range: Range<usize>,
    pub query_syntax_info: QuerySyntaxInfo,
    pub path: PathBuf,
    pub cursor: u32,
}

impl QueryMetadata {
    pub fn new(location: Location, char_range: Range<usize>, info: QuerySyntaxInfo) -> Self {
        Self {
            line: location.line,
            char_range,
            query_syntax_info: info,
            path: location.path.to_path_buf(),
            cursor: location.character,
        }
    }
}

#[derive(Debug, Clone)]
pub struct QuerySyntaxInfo {
    /// Display: If None, there is no display syntax entered; If Some, this is a structure for it
    /// but the string could be empty; for example [[file#heading|]] or even [](file#heaing)
    pub syntax_type_info: QuerySyntaxTypeInfo,
}

impl QuerySyntaxInfo {
    pub fn display(&self) -> Option<&str> {
        match &self.syntax_type_info {
            QuerySyntaxTypeInfo::Markdown { display } => Some(&display),
            QuerySyntaxTypeInfo::Wiki { display } => display.as_deref(),
        }
    }
}

/// This is a plain enum for now, but there may be item specific syntax used. For example, if file
/// extensions are used or if paths are used
#[derive(Debug, PartialEq, Clone)]
pub enum QuerySyntaxTypeInfo {
    Markdown { display: String },
    Wiki { display: Option<String> },
}

impl<'a> MDRegexParseable<'a> for NamedRefCmdQuery<'a> {
    fn from_captures(captures: Captures<'a>) -> Option<Self> {
        let file_ref = captures.name("file_ref")?.as_str();
        let infile_ref = captures
            .name("heading")
            .map(|m| EntityInfileQuery::Heading(m.as_str()))
            .or_else(|| {
                captures
                    .name("index")
                    .map(|m| EntityInfileQuery::Index(m.as_str()))
            });

        Some(NamedRefCmdQuery {
            file_query: file_ref,
            infile_query: infile_ref,
        })
    }

    fn associated_regex_constructor(char_class: &str) -> String {
        format!(
            r"(?<file_ref>{char_class}*?)(#((\^(?<index>{char_class}*?))|(?<heading>{char_class}*?)))??"
        )
    }
}

impl<'a> MDRegexParseable<'a> for BlockLinkCmdQuery {
    fn from_captures(captures: Captures<'a>) -> Option<Self> {
        Some(BlockLinkCmdQuery {
            grep_string: captures.name("grep")?.as_str().to_string(),
        })
    }

    fn associated_regex_constructor(char_class: &str) -> String {
        format!(" (?<grep>{char_class}*?)")
    }
}

mod md_regex_parser {
    use std::ops::Range;

    use regex::{Captures, Regex};

    use super::{QuerySyntaxInfo, QuerySyntaxTypeInfo};

    pub struct MDLinkParser<'a> {
        hay: &'a str,
        character: usize,
    }

    pub trait MDRegexParseable<'a>: Sized {
        fn from_captures(captures: Captures<'a>) -> Option<Self>;
        fn associated_regex_constructor(char_class: &str) -> String;
    }

    impl<'a> MDLinkParser<'a> {
        pub fn new(string: &'a str, character: usize) -> MDLinkParser {
            MDLinkParser {
                hay: string,
                character,
            }
        }

        pub fn parse<T: MDRegexParseable<'a>>(&self) -> Option<(T, Range<usize>, QuerySyntaxInfo)> {
            let link_char = r"(([^\[\]\(\)]|\\)[\[\]]?)"; // Excludes [,],(,), except for when it is escaped

            let query_re = T::associated_regex_constructor(link_char);

            let wiki_re_with_closing = Regex::new(&format!(
                r"\[\[{query_re}(\|(?<display>{link_char}*?))?\]\]"
            ))
            .expect("Regex failed to compile");

            // TODO: consider supporting display text without closing? When would this ever happen??
            let wiki_re_without_closing =
                Regex::new(&format!(r"\[\[{query_re}$")).expect("Regex failed to compile");

            let md_re_with_closing =
                Regex::new(&format!(r"\[(?<display>{link_char}*?)\]\({query_re}\)"))
                    .expect("Regex failed to compile");

            let md_re_without_closing =
                Regex::new(&format!(r"\[(?<display>{link_char}*?)\]\({query_re}$"))
                    .expect("Regex failed to compile");

            let (c, link_type, syntax_type) = wiki_re_with_closing
                .captures_iter(self.hay)
                .find(|c| {
                    c.get(0)
                        .is_some_and(|m| m.range().contains(&self.character))
                })
                .map(|c| (c, ParsedLinkType::Closed, SyntaxType::Wiki))
                .or_else(|| {
                    wiki_re_without_closing
                        .captures_iter(&self.hay[..self.character])
                        .find(|c| c.get(0).is_some_and(|m| m.range().start < self.character))
                        .map(|c| (c, ParsedLinkType::Unclosed, SyntaxType::Wiki))
                })
                .or_else(|| {
                    md_re_with_closing
                        .captures_iter(self.hay)
                        .find(|c| {
                            c.get(0)
                                .is_some_and(|m| m.range().contains(&self.character))
                        })
                        .map(|c| (c, ParsedLinkType::Closed, SyntaxType::Markdown))
                })
                .or_else(|| {
                    md_re_without_closing
                        .captures_iter(&self.hay[..self.character])
                        .find(|c| c.get(0).is_some_and(|m| m.range().start < self.character))
                        .map(|c| (c, ParsedLinkType::Unclosed, SyntaxType::Markdown))
                })?;

            let char_range = c.get(0)?.range().start..(match link_type {
                ParsedLinkType::Closed => c.get(0)?.range().end,
                ParsedLinkType::Unclosed => self.character, // this should be correct because the character is one
                                                            // beyond the last character typed, so it is the exclusive
                                                            // range
            });

            let display = c.name("display").map(|m| m.as_str());

            Some((
                T::from_captures(c)?,
                char_range,
                QuerySyntaxInfo {
                    syntax_type_info: match syntax_type {
                        SyntaxType::Wiki => QuerySyntaxTypeInfo::Wiki {
                            display: display.map(ToString::to_string),
                        },
                        SyntaxType::Markdown => QuerySyntaxTypeInfo::Markdown {
                            display: display
                                .expect("that the display should not be none on markdown link")
                                .to_string(),
                        },
                    },
                },
            ))
        }
    }

    #[derive(Debug)]
    enum ParsedLinkType {
        Closed,
        Unclosed,
    }
    #[derive(Debug, PartialEq)]
    enum SyntaxType {
        Markdown,
        Wiki,
    }
}

#[cfg(test)]
mod named_query_parse_tests {
    use crate::parser::{
        md_regex_parser::MDLinkParser, EntityInfileQuery, NamedRefCmdQuery, QuerySyntaxTypeInfo,
    };

    #[test]
    fn test_file() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file]] jfkdlsa fjdkl ";

        let (parsed, range, ..) = MDLinkParser::new(line, 55 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: None
            }
        );

        assert_eq!(range, 51 - 21..59 - 21)
    }

    #[test]
    fn test_infile_ref_heading() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#heading]] jfkdlsa fjdkl ";

        let (parsed, ..) = MDLinkParser::new(line, 58 - 19)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Heading("heading"))
            }
        )
    }

    #[test]
    fn test_infile_ref_index() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#^index]] fjdlkf jsdakl";

        let (parsed, ..) = MDLinkParser::new(line, 58 - 19)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Index("index"))
            }
        )
    }

    #[test]
    fn test_blank_infile_index() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#^]]";

        let (parsed, ..) = MDLinkParser::new(line, 58 - 19)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Index(""))
            }
        )
    }

    #[test]
    fn test_blank_infile_heading() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#]]";

        let (parsed, ..) = MDLinkParser::new(line, 58 - 22)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Heading(""))
            }
        )
    }

    #[test]
    fn test_no_closing() {
        //                                                         C
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[this is a query jf dkljfa ";

        let (parsed, ..) = MDLinkParser::new(line, 68 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "this is a query",
                infile_query: None
            }
        )
    }

    #[test]
    fn test_markdown_link() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [this is a query](file) jfkdlsa fjdkl ";
        let (parsed, range, info) = MDLinkParser::new(line, 53 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();

        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: None
            }
        );

        assert_eq!(range, 51 - 21..74 - 21);
        assert_eq!(
            info.syntax_type_info,
            QuerySyntaxTypeInfo::Markdown {
                display: "this is a query".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_link_no_closing() {
        //                                                                      C
        let line = "fjlfjdl fjkl lkjfkld fklasj   [this is a query](file jfkldas fjklsd jfkls";
        let (parsed, range, info) = MDLinkParser::new(line, 81 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file jfkldas",
                infile_query: None
            }
        );
        assert_eq!(range, 51 - 21..81 - 21);
        assert_eq!(
            info.syntax_type_info,
            QuerySyntaxTypeInfo::Markdown {
                display: "this is a query".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_closed_infile_query() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [this is a query](file#heading) jfkdlsa fjdkl ";
        let (parsed, range, info) = MDLinkParser::new(line, 63 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Heading("heading"))
            }
        );
        assert_eq!(range, 51 - 21..82 - 21);
        assert_eq!(
            info.syntax_type_info,
            QuerySyntaxTypeInfo::Markdown {
                display: "this is a query".to_string()
            }
        );
    }

    #[test]
    fn test_markdown_closed_infile_query_index() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [this is a query](file#^index) jfkdlsa fjdkl ";
        let (parsed, range, info) = MDLinkParser::new(line, 63 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(
            parsed,
            NamedRefCmdQuery {
                file_query: "file",
                infile_query: Some(EntityInfileQuery::Index("index"))
            }
        );
        assert_eq!(range, 51 - 21..81 - 21);
        assert_eq!(
            info.syntax_type_info,
            QuerySyntaxTypeInfo::Markdown {
                display: "this is a query".to_string()
            }
        );
    }

    #[test]
    fn markdown_syntax_display_text() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [](file#^index) jfkdlsa fjdkl ";
        let (_parsed, _range, info) = MDLinkParser::new(line, 63 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(info.display(), Some(""))
    }

    #[test]
    fn wiki_syntax_display_text_none() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#^index|]] jfkdlsa fjdkl ";
        let (_parsed, _range, info) = MDLinkParser::new(line, 63 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(info.display(), Some(""))
    }

    #[test]
    fn wiki_syntax_display_text_some() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file#^index|some]] jfkdlsa fjdkl ";
        let (_parsed, _range, info) = MDLinkParser::new(line, 63 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(info.display(), Some("some"))
    }

    #[test]
    fn wiki_unclosed_with_multiple_links() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file query jfkdlsa fjdkl [[file#^index|some]]";
        let (parsed, _range, _info) = MDLinkParser::new(line, 71 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(parsed.file_query, "file query jfkdlsa")
    }

    #[test]
    fn wiki_unclosed_after_link() {
        let line = "fjlfjdl fjkl lkjfkld [[link]] fklasj   [[file query jfkdlsa fjdkl";
        let (parsed, _range, _info) = MDLinkParser::new(line, 72 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(parsed.file_query, "file query")
    }

    #[test]
    fn md_unclosed_before_link() {
        let line = "fjlfjdl fjkl lkjfkld [display](file query f sdklafjdkl  j[another linke](file)";
        let (parsed, _range, info) = MDLinkParser::new(line, 62 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(parsed.file_query, "file query");
        assert_eq!(info.display(), Some("display"))
    }

    #[test]
    fn md_unclosed_after_link() {
        let line = "fjlfjdl fjkl lkjfkld [display](file) f sdklafjdkl [another](fjsdklf dsjkl fdj asklfsdjklf ";
        let (parsed, _range, info) = MDLinkParser::new(line, 94 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(parsed.file_query, "fjsdklf dsjkl");
        assert_eq!(info.display(), Some("another"))
    }

    #[test]
    fn wiki_unclosed_with_special_chars() {
        let line = "fjlfjdl fjkl lkjfkld fklasj   [[file query # heading with a # in it and a ^ ajfkl dfkld jlk";
        let (parsed, _, _) = MDLinkParser::new(line, 102 - 21)
            .parse::<NamedRefCmdQuery>()
            .unwrap();
        assert_eq!(parsed.file_query, "file query ");
        assert_eq!(
            parsed.infile_query,
            Some(EntityInfileQuery::Heading(
                " heading with a # in it and a ^ ajfkl"
            ))
        )
    }
}

#[cfg(test)]
mod unnamed_query_tests {
    use crate::parser::{md_regex_parser::MDLinkParser, BlockLinkCmdQuery, NamedRefCmdQuery};

    #[test]
    fn basic_test() {
        let text = "fjkalf kdsjfkd  [[ fjakl fdjk]] fjdl kf j";
        let (d, _, _) = MDLinkParser::new(text, 50 - 21)
            .parse::<BlockLinkCmdQuery>()
            .unwrap();
        assert_eq!("fjakl fdjk", d.grep_string)
    }

    #[test]
    fn unclosed() {
        let text = "fjkalf kdsjfkd  [[ fjakl fdjk fjdl kf j";
        let (d, _, _) = MDLinkParser::new(text, 50 - 21)
            .parse::<BlockLinkCmdQuery>()
            .unwrap();
        assert_eq!("fjakl fdjk", d.grep_string)
    }

    #[test]
    fn multiple_closed() {
        let text = "fjka[[thisis ]] [[ fjakl fdjk]][[fjk]]j";
        let (d, _, _) = MDLinkParser::new(text, 50 - 21)
            .parse::<BlockLinkCmdQuery>()
            .unwrap();
        assert_eq!("fjakl fdjk", d.grep_string)
    }

    #[test]
    fn multiple_unclosed() {
        let text = "fjka[[thisis ]] [[ fjakl fdjk  jklfd slk [[fjk]]j";
        let (d, _, _) = MDLinkParser::new(text, 50 - 21)
            .parse::<BlockLinkCmdQuery>()
            .unwrap();
        assert_eq!("fjakl fdjk", d.grep_string)
    }

    #[test]
    fn not_unnamed_query() {
        let text = "fjka[[thisis ]] [[fjakl fdjkk]]  jklfd slk [[fjk]]j";
        assert!(MDLinkParser::new(text, 50 - 21)
            .parse::<BlockLinkCmdQuery>()
            .is_none())
    }

    #[test]
    fn test_escaped_brackets() {
        let text = r"fjka    [[ \[\[LATER\]\]]]";
        assert_eq!(
            MDLinkParser::new(text, 40 - 22)
                .parse::<BlockLinkCmdQuery>()
                .map(|it| it.0.grep_string()),
            Some(r"[[LATER]]".to_string())
        )
    }

    #[test]
    fn link_with_escaped_braket_display() {
        let text = r"fjka    [[file|\[\[HELLO\]\]]]";

        assert!(MDLinkParser::new(text, 40 - 22)
            .parse::<NamedRefCmdQuery>()
            .map(|it| {
                match it.2.syntax_type_info {
                    crate::parser::QuerySyntaxTypeInfo::Wiki { display: Some(s) } => {
                        &s == r"\[\[HELLO\]\]"
                    }
                    _ => false,
                }
            })
            .is_some_and(|it| it))
    }
}
