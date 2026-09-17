use crate::{
    error::MainReason,
    internal_prelude::*,
    localize::{TemplateConfig, TemplatePath},
};

use crate::prelude::ErrorOwe;
use fs_extra::dir::CopyOptions;
use handlebars::Handlebars;
use log::debug;
use serde::Serialize;
use std::collections::VecDeque;
use std::ffi::OsStr;

const PROTECTED_BEG: &str = "!<!";
const PROTECTED_END: &str = "!>!";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommentFmt {
    CStyle,
    Shell,
    Yml,
    UnNeed,
}

impl From<Option<&std::ffi::OsStr>> for CommentFmt {
    fn from(value: Option<&OsStr>) -> Self {
        match value.and_then(|item| item.to_str()) {
            Some("yml" | "yaml") => Self::Yml,
            Some("sh" | "bash" | "zsh" | "ksh") => Self::Shell,
            Some(
                "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "java" | "js" | "ts" | "rs" | "go"
                | "kt" | "swift" | "css",
            ) => Self::CStyle,
            _ => Self::UnNeed,
        }
    }
}

impl CommentFmt {
    fn remove(self, code: &str) -> String {
        match self {
            CommentFmt::CStyle => strip_c_style_comments(code),
            CommentFmt::Shell => strip_shell_comments(code),
            CommentFmt::Yml => strip_yaml_comments(code),
            CommentFmt::UnNeed => code.to_string(),
        }
    }
}

#[derive(Clone)]
struct LabelCoverter {
    origin: (String, String),
    target: (String, String),
}

impl LabelCoverter {
    fn new(origin: (String, String), target: (String, String)) -> Self {
        Self { origin, target }
    }

    fn convert(&self, fmt: CommentFmt, input: String) -> MainResult<String> {
        let pure = fmt.remove(&input);
        Ok(replace_labels(
            &pure,
            &[
                (&self.target.0, PROTECTED_BEG),
                (&self.target.1, PROTECTED_END),
                (&self.origin.0, &self.target.0),
                (&self.origin.1, &self.target.1),
            ],
        ))
    }

    fn restore(&self, input: String) -> MainResult<String> {
        Ok(replace_labels(
            &input,
            &[
                (&self.target.0, &self.origin.0),
                (&self.target.1, &self.origin.1),
                (PROTECTED_BEG, &self.target.0),
                (PROTECTED_END, &self.target.1),
            ],
        ))
    }
}

enum CustTmplLabel {
    None,
    Setting(LabelCoverter),
}

impl CustTmplLabel {
    fn convert(&self, fmt: CommentFmt, input: String) -> MainResult<String> {
        match self {
            CustTmplLabel::None => Ok(input),
            CustTmplLabel::Setting(convertor) => convertor.convert(fmt, input),
        }
    }

    fn restore(&self, input: String) -> MainResult<String> {
        match self {
            CustTmplLabel::None => Ok(input),
            CustTmplLabel::Setting(convertor) => convertor.restore(input),
        }
    }
}

fn replace_labels(input: &str, replacements: &[(&str, &str)]) -> String {
    let mut output = String::with_capacity(input.len());
    for segment in input.split_inclusive('\n') {
        let (line, has_newline) = match segment.strip_suffix('\n') {
            Some(line) => (line, true),
            None => (segment, false),
        };
        let replaced = replacements
            .iter()
            .fold(line.to_string(), |acc, (from, to)| acc.replace(from, to));
        output.push_str(&replaced);
        if has_newline {
            output.push('\n');
        }
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct YamlBlockState {
    base_indent: usize,
    explicit_indent: Option<usize>,
    required_indent: Option<usize>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum YamlQuoteState {
    None,
    SingleQuoted,
    DoubleQuoted { escape: bool },
}

fn strip_c_style_comments(input: &str) -> String {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum State {
        Code,
        LineComment,
        BlockComment,
        DoubleQuoted,
        SingleQuoted,
        BacktickQuoted,
    }

    let mut output = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut state = State::Code;
    let mut escape = false;
    let mut index = 0;

    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();
        match state {
            State::Code => {
                if ch == '/' && next == Some('/') {
                    state = State::LineComment;
                    index += 2;
                    continue;
                }
                if ch == '/' && next == Some('*') {
                    state = State::BlockComment;
                    index += 2;
                    continue;
                }
                match ch {
                    '"' => state = State::DoubleQuoted,
                    '\'' => state = State::SingleQuoted,
                    '`' => state = State::BacktickQuoted,
                    _ => {}
                }
                output.push(ch);
                index += 1;
            }
            State::LineComment => {
                if ch == '\n' {
                    output.push('\n');
                    state = State::Code;
                }
                index += 1;
            }
            State::BlockComment => {
                if ch == '*' && next == Some('/') {
                    state = State::Code;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            State::DoubleQuoted => {
                output.push(ch);
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    state = State::Code;
                }
                index += 1;
            }
            State::SingleQuoted => {
                output.push(ch);
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '\'' {
                    state = State::Code;
                }
                index += 1;
            }
            State::BacktickQuoted => {
                output.push(ch);
                if ch == '`' {
                    state = State::Code;
                }
                index += 1;
            }
        }
    }

    output
}

fn strip_shell_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut state = ShellQuoteState::Code;
    let mut brace_depth = 0usize;
    let mut heredocs = VecDeque::new();

    for segment in input.split_inclusive('\n') {
        let (line, has_newline) = match segment.strip_suffix('\n') {
            Some(line) => (line, true),
            None => (segment, false),
        };

        if let Some(active) = heredocs.front() {
            output.push_str(line);
            if has_newline {
                output.push('\n');
            }
            if shell_heredoc_terminates(line, active) {
                heredocs.pop_front();
            }
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut index = 0usize;
        let mut line_start = true;
        let mut line_output = String::with_capacity(line.len());
        let mut pending_heredocs = Vec::new();

        while index < chars.len() {
            let ch = chars[index];
            let next = chars.get(index + 1).copied();
            let next_next = chars.get(index + 2).copied();

            match state {
                ShellQuoteState::Code => {
                    if ch == '$' && next == Some('(') && next_next == Some('(') {
                        state = ShellQuoteState::Arithmetic { paren_depth: 0 };
                        line_output.push(ch);
                        line_output.push('(');
                        line_output.push('(');
                        line_start = false;
                        index += 3;
                        continue;
                    }
                    if ch == '\\' {
                        line_output.push(ch);
                        if let Some(next) = next {
                            line_output.push(next);
                            line_start = next == '\n';
                            index += 2;
                        } else {
                            index += 1;
                        }
                        continue;
                    }
                    if ch == '$' && next == Some('{') {
                        brace_depth += 1;
                        line_output.push(ch);
                        line_output.push('{');
                        line_start = false;
                        index += 2;
                        continue;
                    }
                    if ch == '}' && brace_depth > 0 {
                        brace_depth -= 1;
                        line_output.push(ch);
                        line_start = false;
                        index += 1;
                        continue;
                    }
                    if ch == '<'
                        && next == Some('<')
                        && next_next != Some('<')
                        && brace_depth == 0
                        && let Some((consumed, found)) = parse_shell_heredoc(&chars[index..])
                    {
                        line_output.extend(chars[index..index + consumed].iter());
                        if let Some(found) = found {
                            pending_heredocs.push(found);
                        }
                        line_start = false;
                        index += consumed;
                        continue;
                    }
                    if ch == '#'
                        && brace_depth == 0
                        && shell_comment_starts(line_start, line_output.chars().last())
                    {
                        break;
                    }
                    match ch {
                        '\'' => state = ShellQuoteState::SingleQuoted,
                        '"' => state = ShellQuoteState::DoubleQuoted { escape: false },
                        '`' => state = ShellQuoteState::BacktickQuoted,
                        _ if !ch.is_whitespace() => line_start = false,
                        _ => {}
                    }
                    line_output.push(ch);
                    index += 1;
                }
                ShellQuoteState::SingleQuoted => {
                    line_output.push(ch);
                    if ch == '\'' {
                        state = ShellQuoteState::Code;
                    }
                    index += 1;
                }
                ShellQuoteState::DoubleQuoted { escape } => {
                    line_output.push(ch);
                    if escape {
                        state = ShellQuoteState::DoubleQuoted { escape: false };
                    } else if ch == '\\' {
                        state = ShellQuoteState::DoubleQuoted { escape: true };
                    } else if ch == '"' {
                        state = ShellQuoteState::Code;
                    }
                    index += 1;
                }
                ShellQuoteState::BacktickQuoted => {
                    line_output.push(ch);
                    if ch == '`' {
                        state = ShellQuoteState::Code;
                    }
                    index += 1;
                }
                ShellQuoteState::Arithmetic { mut paren_depth } => {
                    line_output.push(ch);
                    if ch == '(' {
                        paren_depth += 1;
                        state = ShellQuoteState::Arithmetic { paren_depth };
                        index += 1;
                        continue;
                    }
                    if ch == ')' {
                        if paren_depth == 0 && next == Some(')') {
                            line_output.push(')');
                            state = ShellQuoteState::Code;
                            index += 2;
                            continue;
                        }
                        paren_depth = paren_depth.saturating_sub(1);
                        state = ShellQuoteState::Arithmetic { paren_depth };
                    }
                    index += 1;
                }
            }
        }

        output.push_str(&line_output);
        if has_newline {
            output.push('\n');
        }
        if !pending_heredocs.is_empty() {
            heredocs.extend(pending_heredocs);
        }
    }

    output
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellHereDoc {
    delimiter: String,
    allow_tab_indent: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShellQuoteState {
    Code,
    SingleQuoted,
    DoubleQuoted { escape: bool },
    BacktickQuoted,
    Arithmetic { paren_depth: usize },
}

fn parse_shell_heredoc(chars: &[char]) -> Option<(usize, Option<ShellHereDoc>)> {
    if chars.first() != Some(&'<') || chars.get(1) != Some(&'<') {
        return None;
    }

    let mut index = 2usize;
    let mut allow_tab_indent = false;
    if chars.get(index) == Some(&'-') {
        allow_tab_indent = true;
        index += 1;
    }

    while chars.get(index).is_some_and(|ch| ch.is_whitespace()) {
        index += 1;
    }

    let Some(first) = chars.get(index).copied() else {
        return Some((index, None));
    };

    let delimiter = if matches!(first, '\'' | '"') {
        let quote = first;
        index += 1;
        let start = index;
        while chars.get(index).is_some_and(|ch| *ch != quote) {
            index += 1;
        }
        let delimiter: String = chars[start..index].iter().collect();
        if chars.get(index) == Some(&quote) {
            index += 1;
        }
        delimiter
    } else {
        let start = index;
        while chars
            .get(index)
            .is_some_and(|ch| !ch.is_whitespace() && !matches!(ch, ';' | '|' | '&'))
        {
            index += 1;
        }
        chars[start..index].iter().collect()
    };

    if delimiter.is_empty() {
        return Some((index, None));
    }

    Some((
        index,
        Some(ShellHereDoc {
            delimiter,
            allow_tab_indent,
        }),
    ))
}

fn shell_heredoc_terminates(line: &str, heredoc: &ShellHereDoc) -> bool {
    let candidate = if heredoc.allow_tab_indent {
        line.trim_start_matches('\t')
    } else {
        line
    };
    candidate == heredoc.delimiter
}

fn shell_comment_starts(line_start: bool, previous: Option<char>) -> bool {
    line_start
        || previous.is_none_or(|ch| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    ';' | '|' | '&' | '(' | ')' | '{' | '}' | '<' | '>' | ':'
                )
        })
}

fn strip_yaml_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut block_state: Option<YamlBlockState> = None;
    let mut quote_state = YamlQuoteState::None;

    for segment in input.split_inclusive('\n') {
        let (line, has_newline) = match segment.strip_suffix('\n') {
            Some(line) => (line, true),
            None => (segment, false),
        };

        let current_indent = line.chars().take_while(|ch| ch.is_whitespace()).count();
        if let Some(state) = block_state.as_mut() {
            if line.trim().is_empty() {
                output.push_str(line);
                if has_newline {
                    output.push('\n');
                }
                continue;
            }

            let required_indent = if let Some(indent) = state.required_indent {
                indent
            } else if let Some(indent) = state.explicit_indent {
                state.required_indent = Some(indent);
                indent
            } else if current_indent > state.base_indent {
                state.required_indent = Some(current_indent);
                current_indent
            } else {
                block_state = None;
                0
            };

            if block_state.is_some() && current_indent >= required_indent {
                output.push_str(line);
                if has_newline {
                    output.push('\n');
                }
                continue;
            }

            block_state = None;
        }

        let stripped = strip_yaml_comment_line(line, &mut quote_state);
        if let Some(state) = yaml_block_scalar_state(&stripped) {
            block_state = Some(state);
        }
        output.push_str(&stripped);
        if has_newline {
            output.push('\n');
        }
    }

    output
}

fn strip_yaml_comment_line(line: &str, quote_state: &mut YamlQuoteState) -> String {
    let mut output = String::with_capacity(line.len());
    let mut iter = line.chars().peekable();

    while let Some(ch) = iter.next() {
        match quote_state {
            YamlQuoteState::SingleQuoted => {
                output.push(ch);
                if ch == '\'' && iter.peek() == Some(&'\'') {
                    output.push('\'');
                    iter.next();
                } else if ch == '\'' {
                    *quote_state = YamlQuoteState::None;
                }
                continue;
            }
            YamlQuoteState::DoubleQuoted { escape } => {
                output.push(ch);
                if *escape {
                    *quote_state = YamlQuoteState::DoubleQuoted { escape: false };
                } else if ch == '\\' {
                    *quote_state = YamlQuoteState::DoubleQuoted { escape: true };
                } else if ch == '"' {
                    *quote_state = YamlQuoteState::None;
                }
                continue;
            }
            YamlQuoteState::None => match ch {
                '\'' => {
                    *quote_state = YamlQuoteState::SingleQuoted;
                    output.push(ch);
                }
                '"' => {
                    *quote_state = YamlQuoteState::DoubleQuoted { escape: false };
                    output.push(ch);
                }
                '#' if yaml_comment_starts(&output) => break,
                _ => output.push(ch),
            },
        }
    }

    output
}

fn yaml_comment_starts(output: &str) -> bool {
    output.is_empty() || output.chars().last().is_some_and(char::is_whitespace)
}

fn yaml_block_scalar_state(line: &str) -> Option<YamlBlockState> {
    let trimmed = line.trim_end();
    let marker = trimmed.split_whitespace().last()?;
    let explicit_indent = parse_yaml_block_scalar_marker(marker)?;
    Some(YamlBlockState {
        base_indent: line.chars().take_while(|ch| ch.is_whitespace()).count(),
        explicit_indent: explicit_indent
            .map(|indent| line.chars().take_while(|ch| ch.is_whitespace()).count() + indent),
        required_indent: None,
    })
}

fn parse_yaml_block_scalar_marker(marker: &str) -> Option<Option<usize>> {
    let mut chars = marker.chars();
    let style = chars.next()?;
    if !matches!(style, '|' | '>') {
        return None;
    }

    let mut indent: Option<usize> = None;
    for ch in chars {
        match ch {
            '+' | '-' => {}
            '1'..='9' => indent = ch.to_digit(10).map(|digit| digit as usize),
            _ => return None,
        }
    }

    Some(indent)
}

pub struct TplHandleBars<'a> {
    handlebars: Handlebars<'a>,
}
impl TplHandleBars<'_> {
    pub fn init() -> Self {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        handlebars.register_escape_fn(handlebars::no_escape);
        Self { handlebars }
    }

    pub fn render_data<T: Serialize>(&self, template: &str, data: &T) -> MainResult<String> {
        let out_data = self
            .handlebars
            .render_template(template, data)
            .source_biz()?;
        Ok(out_data)
    }
}

pub struct LocalizeTemplate<'a> {
    handlebars: TplHandleBars<'a>,
    cust_cover: CustTmplLabel,
}
impl Default for LocalizeTemplate<'_> {
    fn default() -> Self {
        Self {
            handlebars: TplHandleBars::init(),
            cust_cover: CustTmplLabel::None,
        }
    }
}
impl LocalizeTemplate<'_> {
    pub fn new(cust: TemplateConfig) -> Self {
        let convert = LabelCoverter::new(cust.origin().clone(), cust.target().clone());
        Self {
            handlebars: TplHandleBars::init(),
            cust_cover: CustTmplLabel::Setting(convert),
        }
    }
}
impl LocalizeTemplate<'_> {
    pub fn render_path(
        &self,
        tpl: &PathBuf,
        dst: &PathBuf,
        data: &PathBuf,
        setting: &TemplatePath,
    ) -> MainResult<()> {
        let mut err_ctx = WithContext::want("render tpl path");
        // 处理目录模板
        err_ctx.record("data", data.display());
        let content = std::fs::read_to_string(data).source_data().with(&err_ctx)?;
        err_ctx.record("need-fmt", "json");
        let data: serde_json::Value = serde_json::from_str(content.as_str())
            .source_data()
            .with(&err_ctx)?;
        if tpl.is_dir() {
            self.render_dir_impl(tpl, dst, &data, setting)
                .with(&err_ctx)
        } else {
            self.render_file_impl(tpl, dst, &data, setting)
                .with(&err_ctx)
        }
    }

    fn render_dir_impl<T: Serialize>(
        &self,
        tpl_dir: &PathBuf,
        dst: &PathBuf,
        data: &T,
        setting: &TemplatePath,
    ) -> MainResult<()> {
        debug!("tpl dir: {}", tpl_dir.display());
        for entry in walkdir::WalkDir::new(tpl_dir) {
            let entry = entry.source_data()?;
            let tpl_path = entry.path().to_path_buf();
            let relative_path = tpl_path.strip_prefix(tpl_dir).source_data()?;
            let dst_path = Path::new(dst).join(relative_path);

            if tpl_path.is_dir() {
                // 如果是目录，确保在目标位置创建对应的目录
                std::fs::create_dir_all(&dst_path).source_sys()?;
                debug!("created dir: {}", dst_path.display());
            } else if tpl_path.is_file() {
                // 如果是文件，则渲染模板
                self.render_file_impl(&tpl_path, &dst_path, &data, setting)?;
            }
        }
        Ok(())
    }

    fn render_file_impl<T: Serialize>(
        &self,
        tpl_path: &PathBuf,
        dst_path: &PathBuf,
        data: &T,
        templatize: &TemplatePath,
    ) -> MainResult<()> {
        debug!("tpl:{}", tpl_path.display());
        debug!("dst:{}", dst_path.display());

        let mut err_ctx = WithContext::want("render tpl");
        err_ctx.record("tpl", tpl_path.display());
        // 2. 验证模板文件
        if !tpl_path.exists() {
            return Err(MainReason::conf_detail(format!(
                "tpl path not exists: {}",
                tpl_path.display()
            )))
            .with(&err_ctx);
        }
        if !templatize.is_include(tpl_path) {
            info!("ignore:{}", tpl_path.display());
            return Ok(());
        }
        if templatize.is_exclude(tpl_path) {
            if let Some(dist) = dst_path.parent() {
                println!("copy {:30} ---> {}", tpl_path.display(), dist.display());
                fs_extra::copy_items(&[&tpl_path], dist, &CopyOptions::default())
                    .source_resource()
                    .with(("tpl", tpl_path))
                    .with(("dst", dist))?;

                return Ok(());
            }
            return Err(MainReason::resource_detail(format!(
                "path has no parent: {}",
                dst_path.display()
            )))
            .with(dst_path);
        }
        err_ctx.record("dst", dst_path.display());

        // 3. 准备目标文件
        let dst_path = Path::new(&dst_path);
        if let Some(parent) = dst_path.parent() {
            std::fs::create_dir_all(parent).source_sys()?;
        }
        if dst_path.exists() {
            std::fs::remove_file(dst_path).source_sys()?;
        }

        // 4. 日志记录
        debug!(
            "Processing template: {} → {}",
            tpl_path.display(),
            dst_path.display()
        );

        // 5. 读取模板内容
        let template = std::fs::read_to_string(tpl_path)
            .source_data()
            .with(&err_ctx)?;

        //let convert = TplCoverter::new("[[", "]]", "{{", "}}", CommentLabel::yml_style());
        let template = self
            .cust_cover
            .convert(CommentFmt::from(tpl_path.extension()), template)
            .with(&err_ctx)?;
        //let mut dst_file = File::create(dst_path).owe_conf()?;

        let rendered_data = self
            .handlebars
            //.render_template_to_write(&template, data, &mut dst_file)
            .render_data(&template, data)
            .source_biz()
            .with(&err_ctx)?;
        let completed = self.cust_cover.restore(rendered_data).with(&err_ctx)?;
        std::fs::write(dst_path, completed)
            .source_conf()
            .with(dst_path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o644); // rw-r--r--
            std::fs::set_permissions(dst_path, perms)
                .source_sys()
                .with(&err_ctx)?;
        }
        println!(
            "render {:30} ---> {}",
            tpl_path.display(),
            dst_path.display()
        );

        debug!("Successfully generated: {}", dst_path.display());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_error::dev::testing::TestAssert;
    use tempfile::tempdir;

    #[test]
    fn test_render_path_with_handlebars() {
        // 准备测试目录结构
        let tmp_dir = tempdir().unwrap();
        let tpl_dir = tmp_dir.path().join("templates");
        std::fs::create_dir_all(&tpl_dir).unwrap();

        // 创建测试模板文件
        let tpl_file = tpl_dir.join("test.hbs");
        std::fs::write(&tpl_file, "Hello, {{name}}!").unwrap();

        // 创建测试数据文件
        let data_file = tmp_dir.path().join("data.json");
        std::fs::write(&data_file, r#"{"name": "&World"}"#).unwrap();

        // 准备输出目录
        let output_dir = tmp_dir.path().join("output");

        // 空白的模板路径设置
        let setting = TemplatePath::default();

        // 执行渲染
        let result = LocalizeTemplate::default().render_path(
            &tpl_file,
            &output_dir.join("output.txt"),
            &data_file,
            &setting,
        );

        assert!(result.is_ok());

        // 验证输出内容
        let output = std::fs::read_to_string(output_dir.join("output.txt")).unwrap();
        assert_eq!(output, "Hello, &World!");
    }

    #[test]
    fn test_render_directory() {
        let tmp_dir = tempdir().unwrap().path().to_path_buf();
        let tpl_dir = tmp_dir.join("templates");
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir).unwrap();
        }
        std::fs::create_dir_all(tpl_dir.join("subdir")).unwrap();

        // 创建多个模板文件
        std::fs::write(tpl_dir.join("main.hbs"), "Main: {{title}}").unwrap();
        std::fs::write(tpl_dir.join("subdir/file.hbs"), "Sub: {{title}}").unwrap();

        // 数据文件
        let data_file = tmp_dir.join("data.json");
        std::fs::write(&data_file, r#"{"title": "Test"}"#).unwrap();

        let output_dir = tmp_dir.join("output");
        let setting = TemplatePath::default();

        let result =
            LocalizeTemplate::default().render_path(&tpl_dir, &output_dir, &data_file, &setting);

        assert!(result.is_ok());
        assert_eq!(
            std::fs::read_to_string(output_dir.join("main.hbs")).unwrap(),
            "Main: Test"
        );
        assert_eq!(
            std::fs::read_to_string(output_dir.join("subdir/file.hbs")).unwrap(),
            "Sub: Test"
        );
    }

    #[test]
    fn test_excluded_files() {
        let tmp_dir = tempdir().unwrap().path().to_path_buf(); //PathBuf::from("./temp/tpl2");
        let tpl_dir = tmp_dir.join("templates");
        if tmp_dir.exists() {
            std::fs::remove_dir_all(&tmp_dir).unwrap();
        }
        std::fs::create_dir_all(&tpl_dir).unwrap();

        // 创建包含和不包含的文件
        std::fs::write(tpl_dir.join("render.hbs"), "{{content}}").unwrap();
        std::fs::write(tpl_dir.join("exclude.txt"), "raw content").unwrap();

        let data_file = tmp_dir.join("data.json");
        std::fs::write(&data_file, r#"{"content": "test"}"#).unwrap();

        let output_dir = tmp_dir.join("output");

        // 设置排除规则
        let mut setting = TemplatePath::default();
        setting.exclude_mut().push(tpl_dir.join("exclude.txt"));

        LocalizeTemplate::default()
            .render_path(&tpl_dir, &output_dir, &data_file, &setting)
            .unwrap();

        // 验证模板文件被渲染
        assert_eq!(
            std::fs::read_to_string(output_dir.join("render.hbs")).unwrap(),
            "test"
        );
        // 验证排除文件被直接复制
        assert_eq!(
            std::fs::read_to_string(output_dir.join("exclude.txt")).unwrap(),
            "raw content"
        );
    }

    #[test]
    fn test_helm_nginx_rendering() {
        let root_dir = PathBuf::from("./test_data/helm");
        let helm_dir = PathBuf::from("./test_data/helm/nginx");
        let out_dir = PathBuf::from("./test_data/temp/nginx");
        if out_dir.exists() {
            std::fs::remove_dir_all(&out_dir).assert();
        }

        let mut setting = TemplatePath::default();
        setting.exclude_mut().push(helm_dir.join("templates"));

        let cust = TemplateConfig::example();

        //let _result = LocalizeTemplate::default()
        LocalizeTemplate::new(cust)
            .render_path(
                &helm_dir,
                &out_dir,
                &root_dir.join("value.json"), // 使用 values.yaml 作为数据源
                &setting,
            )
            .assert();
    }

    #[test]
    fn test_custom_labels_preserve_existing_target_tags() {
        let tmp_dir = tempdir().unwrap();
        let tpl_file = tmp_dir.path().join("values.yaml");
        let data_file = tmp_dir.path().join("data.json");
        let out_file = tmp_dir.path().join("output.yaml");

        std::fs::write(
            &tpl_file,
            "name: [[name]]\nhelm: {{ .Values.image.tag }}\n# [[ignored_comment]]\n",
        )
        .unwrap();
        std::fs::write(&data_file, r#"{"name":"demo"}"#).unwrap();

        LocalizeTemplate::new(TemplateConfig::example())
            .render_path(&tpl_file, &out_file, &data_file, &TemplatePath::default())
            .assert();

        let output = std::fs::read_to_string(out_file).unwrap();
        assert!(output.contains("name: demo"));
        assert!(output.contains("helm: {{ .Values.image.tag }}"));
        assert!(!output.contains("ignored_comment"));
    }

    #[test]
    fn test_shell_comments_keep_parameter_expansion_and_url_fragments() {
        let input = r#"echo ${name#prod}
URL=http://example.com/#frag
echo done # comment
"#;

        let output = strip_shell_comments(input);

        assert!(output.contains("${name#prod}"));
        assert!(output.contains("http://example.com/#frag"));
        assert!(output.contains("echo done "));
        assert!(!output.contains("# comment"));
    }

    #[test]
    fn test_shell_comments_keep_hash_in_assignment_values() {
        let input = r#"TOKEN=#abc123
export COLOR=#fff
echo ok # trailing comment
"#;

        let output = strip_shell_comments(input);

        assert!(output.contains("TOKEN=#abc123"));
        assert!(output.contains("export COLOR=#fff"));
        assert!(output.contains("echo ok "));
        assert!(!output.contains("# trailing comment"));
    }

    #[test]
    fn test_shell_comments_keep_heredoc_body() {
        let input = r#"cat <<'EOF'
# keep this line
value=${name#prod}
EOF
echo done # comment
"#;

        let output = strip_shell_comments(input);

        assert!(output.contains("# keep this line"));
        assert!(output.contains("value=${name#prod}"));
        assert!(output.contains("EOF"));
        assert!(output.contains("echo done "));
        assert!(!output.contains("# comment"));
    }

    #[test]
    fn test_shell_comments_keep_heredoc_body_with_trailing_comment() {
        let input = r#"cat <<'EOF' # strip this comment
# keep this line
EOF
"#;

        let output = strip_shell_comments(input);

        assert!(output.contains("cat <<'EOF' "));
        assert!(output.contains("# keep this line"));
        assert!(!output.contains("# strip this comment"));
    }

    #[test]
    fn test_shell_comments_keep_tab_stripped_heredoc_body() {
        let input = "cat <<-EOF\n\t# keep this line\n\tEOF\n";

        let output = strip_shell_comments(input);

        assert!(output.contains("cat <<-EOF"));
        assert!(output.contains("\t# keep this line"));
        assert!(output.contains("\tEOF"));
    }

    #[test]
    fn test_shell_comments_do_not_treat_arithmetic_shift_as_heredoc() {
        let input = "echo $((1 << 2)) # remove\nnext=value # remove too\n";

        let output = strip_shell_comments(input);

        assert!(output.contains("echo $((1 << 2)) "));
        assert!(output.contains("next=value "));
        assert!(!output.contains("# remove"));
        assert!(!output.contains("# remove too"));
    }

    #[test]
    fn test_shell_comments_keep_multiple_heredoc_bodies() {
        let input =
            "cat <<'EOF1' <<'EOF2'\n# keep first\nEOF1\n# keep second\nEOF2\necho done # trim\n";

        let output = strip_shell_comments(input);

        assert!(output.contains("# keep first"));
        assert!(output.contains("# keep second"));
        assert!(output.contains("echo done "));
        assert!(!output.contains("# trim"));
    }

    #[test]
    fn test_yaml_comments_require_whitespace_separator() {
        let input = r#"value: abc#def
url: https://example.com/#frag
commented: value # remove
"#;

        let output = strip_yaml_comments(input);

        assert!(output.contains("value: abc#def"));
        assert!(output.contains("url: https://example.com/#frag"));
        assert!(output.contains("commented: value "));
        assert!(!output.contains("# remove"));
    }

    #[test]
    fn test_yaml_block_scalar_with_explicit_indent_keeps_hash_content() {
        let input = "script: |2-\n    line1 # keep\n    ${name#prod}\nnext: value # trim\n";

        let output = strip_yaml_comments(input);

        assert!(output.contains("    line1 # keep"));
        assert!(output.contains("    ${name#prod}"));
        assert!(output.contains("next: value "));
        assert!(!output.contains("# trim"));
    }

    #[test]
    fn test_yaml_multiline_double_quoted_scalar_keeps_hash_lines() {
        let input = "message: \"line1\n# keep this line\nline3\"\nmeta: value # trim\n";

        let output = strip_yaml_comments(input);

        assert!(output.contains("message: \"line1\n# keep this line\nline3\""));
        assert!(output.contains("meta: value "));
        assert!(!output.contains("# trim"));
    }
}
