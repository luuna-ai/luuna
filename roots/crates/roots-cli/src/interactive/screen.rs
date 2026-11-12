//! Pure renderer for the left-hand `think` screen.

use roots_core::color::Colors;
use roots_core::model::EdgeRel;

pub const MIN_WIDTH: usize = 32;
pub const MAX_WIDTH: usize = 100;
const EDGE_LABEL_WIDTH: usize = 11;
const KEY_GAP: usize = 2;
const FINDING_LINES: usize = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenEdge {
    pub rel: EdgeRel,
    pub outgoing: bool,
    pub id: String,
    pub slug: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenFinding {
    pub text: String,
    pub path: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenQuestion {
    pub id: String,
    pub text: String,
    pub finding: Option<ScreenFinding>,
    pub source: String,
    pub index: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenGuidance {
    pub label: String,
    pub source: String,
    pub text: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScreenView {
    pub id: String,
    pub slug: String,
    pub status: String,
    pub statement: String,
    pub edges: Vec<ScreenEdge>,
    pub question: Option<ScreenQuestion>,
    pub notice: Option<String>,
    pub editor_hint: String,
    pub can_delegate: bool,
    pub guidance: Option<ScreenGuidance>,
}

#[derive(Clone, Copy)]
enum Style {
    Plain,
    Dim,
    Bold,
    Green,
    Cyan,
    Id,
    IdBold,
}

#[derive(Clone)]
struct Segment {
    text: String,
    style: Style,
}

impl Segment {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::Plain,
        }
    }
    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

fn paint(colors: Colors, style: Style, text: &str) -> String {
    match style {
        Style::Plain => text.to_owned(),
        Style::Dim => colors.dim(text),
        Style::Bold => colors.bold(text),
        Style::Green => colors.green(text),
        Style::Cyan => colors.cyan(text),
        Style::Id => colors.id(text),
        Style::IdBold => colors.id(&colors.bold(text)),
    }
}

#[must_use]
pub fn truncate(text: &str, max: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max {
        text.to_owned()
    } else if max == 0 {
        String::new()
    } else {
        format!("{}…", chars[..max - 1].iter().collect::<String>())
    }
}

fn wrap_paragraph(paragraph: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut output = Vec::new();
    let mut current = String::new();
    for word in paragraph.split_whitespace() {
        let characters = word.chars().collect::<Vec<_>>();
        for chunk in characters.chunks(width) {
            let piece = chunk.iter().collect::<String>();
            let next = if current.is_empty() {
                piece.clone()
            } else {
                format!("{current} {piece}")
            };
            if next.chars().count() <= width {
                current = next;
            } else {
                if !current.is_empty() {
                    output.push(std::mem::take(&mut current));
                }
                current = piece;
            }
        }
    }
    output.push(current);
    output
}

/// Word-wrap, preserving explicit source newlines and hard-wrapping long words.
#[must_use]
pub fn wrap_text(text: &str, width: usize) -> Vec<String> {
    text.split('\n')
        .flat_map(|paragraph| wrap_paragraph(paragraph, width))
        .collect()
}

fn fit(parts: &[Segment], width: usize, colors: Colors) -> String {
    let mut used = 0;
    let mut output = String::new();
    for part in parts {
        let room = width.saturating_sub(used);
        if room == 0 {
            break;
        }
        let length = part.text.chars().count();
        let text = if length > room {
            truncate(&part.text, room)
        } else {
            part.text.clone()
        };
        used += text.chars().count();
        output.push_str(&paint(colors, part.style, &text));
    }
    output.push_str(&" ".repeat(width.saturating_sub(used)));
    output
}

fn edge_label(edge: &ScreenEdge) -> (&'static str, &'static str) {
    if edge.rel == EdgeRel::Tension {
        return ("tension", "↔");
    }
    if edge.outgoing {
        return (edge.rel.as_str(), "→");
    }
    match edge.rel {
        EdgeRel::Serves => ("served by", "←"),
        EdgeRel::Replaces => ("replaced by", "←"),
        EdgeRel::Derives => ("adopted as", "←"),
        EdgeRel::Tension => unreachable!(),
    }
}

fn divider(label: &str, source: Option<&str>, inner: usize) -> Vec<Segment> {
    let head = format!(
        "── {label} ──{}",
        source.map_or_else(|| " ".to_owned(), |source| format!(" [{source}] "))
    );
    vec![Segment::styled(
        format!(
            "{head}{}",
            "─".repeat(inner.saturating_sub(head.chars().count()))
        ),
        Style::Dim,
    )]
}

fn header(view: &ScreenView, inner: usize) -> Vec<Segment> {
    let room = inner
        .saturating_sub(view.id.len() + view.status.len() + 4)
        .max(1);
    let slug = truncate(&view.slug, room);
    let gap = inner
        .saturating_sub(view.id.len() + 2 + slug.chars().count() + view.status.len())
        .max(2);
    vec![
        Segment::styled(view.id.clone(), Style::IdBold),
        Segment::plain(format!("  {slug}")),
        Segment::plain(" ".repeat(gap)),
        Segment::styled(view.status.clone(), Style::Cyan),
    ]
}

fn edge_rows(view: &ScreenView) -> Vec<Vec<Segment>> {
    if view.edges.is_empty() {
        return vec![vec![Segment::styled("(no links yet)", Style::Dim)]];
    }
    view.edges
        .iter()
        .map(|edge| {
            let (label, arrow) = edge_label(edge);
            vec![
                Segment::plain(format!("{label:<EDGE_LABEL_WIDTH$}{arrow} ")),
                Segment::styled(edge.id.clone(), Style::Id),
                Segment::plain(format!(" {}", edge.slug)),
            ]
        })
        .collect()
}

fn finding_rows(finding: Option<&ScreenFinding>, inner: usize) -> Vec<Vec<Segment>> {
    let Some(finding) = finding else {
        return Vec::new();
    };
    let mut lines = wrap_text(&finding.text, inner);
    if lines.len() > FINDING_LINES {
        lines.truncate(FINDING_LINES);
        let tail = lines.pop().unwrap_or_default();
        lines.push(truncate(&format!("{tail} …"), inner));
    }
    let mut rows = vec![Vec::new(), divider("findings", Some("agent"), inner)];
    rows.extend(
        lines
            .into_iter()
            .map(|line| vec![Segment::styled(line, Style::Dim)]),
    );
    rows.push(vec![Segment::styled(
        truncate(&format!("full: {}", finding.path), inner),
        Style::Dim,
    )]);
    rows
}

fn question_rows(view: &ScreenView, inner: usize) -> Vec<Vec<Segment>> {
    let Some(question) = &view.question else {
        let mut rows = vec![divider("no questions", None, inner)];
        rows.extend(
            wrap_text(
                "Nothing to ask right now. Write freely; saves are kept.",
                inner,
            )
            .into_iter()
            .map(|line| vec![Segment::styled(line, Style::Dim)]),
        );
        return rows;
    };
    let mut rows = vec![divider(
        &format!(
            "question {}/{} · {}",
            question.index, question.total, question.id
        ),
        Some(&question.source),
        inner,
    )];
    rows.extend(
        wrap_text(&question.text, inner)
            .into_iter()
            .map(|line| vec![Segment::styled(line, Style::Bold)]),
    );
    rows.extend(finding_rows(question.finding.as_ref(), inner));
    rows
}

fn guidance_rows(view: &ScreenView, inner: usize) -> Vec<Vec<Segment>> {
    let Some(guidance) = &view.guidance else {
        return Vec::new();
    };
    let mut rows = vec![divider(&guidance.label, Some(&guidance.source), inner)];
    rows.extend(
        wrap_text(&guidance.text, inner)
            .into_iter()
            .map(|line| vec![Segment::styled(line, Style::Dim)]),
    );
    rows.push(Vec::new());
    rows
}

fn column_widths(cells: &[String], columns: usize) -> Vec<usize> {
    let mut widths = vec![0; columns];
    for (index, cell) in cells.iter().enumerate() {
        let last = index % columns == columns - 1 || index == cells.len() - 1;
        if !last {
            widths[index % columns] = widths[index % columns].max(cell.len());
        }
    }
    widths
}

fn grid_width(cells: &[String], widths: &[usize]) -> usize {
    cells
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let column = index % widths.len();
            let offset = widths[..column].iter().sum::<usize>() + KEY_GAP * column;
            offset + cell.len()
        })
        .max()
        .unwrap_or_default()
}

fn key_grid(cells: &[String], inner: usize) -> Vec<usize> {
    for columns in (2..=3.min(cells.len())).rev() {
        let widths = column_widths(cells, columns);
        if grid_width(cells, &widths) <= inner {
            return widths;
        }
    }
    vec![0]
}

fn key_rows(view: &ScreenView, inner: usize) -> Vec<Vec<Segment>> {
    let mut keys: Vec<(&str, &str)> = if view.question.is_some() {
        vec![
            ("save", "answer"),
            ("d", "dismiss"),
            ("z", "snooze"),
            ("s", "skip"),
        ]
    } else {
        Vec::new()
    };
    if view.question.is_some() && view.can_delegate {
        keys.push(("a", "ask agent"));
    }
    keys.push(("q", "end session"));
    let cells = keys
        .iter()
        .map(|(key, label)| format!("[{key}] {label}"))
        .collect::<Vec<_>>();
    let widths = key_grid(&cells, inner);
    let mut rows = Vec::<Vec<Segment>>::new();
    for (index, (key, label)) in keys.iter().enumerate() {
        let column = index % widths.len();
        if column == 0 {
            rows.push(Vec::new());
        }
        let last = column == widths.len() - 1 || index == keys.len() - 1;
        let used = key.len() + label.len() + 3;
        let padding = if last {
            0
        } else {
            widths[column] + KEY_GAP - used
        };
        let row = rows.last_mut().expect("pushed for first column");
        row.push(Segment::styled(format!("[{key}]"), Style::Cyan));
        row.push(Segment::plain(format!(" {label}{}", " ".repeat(padding))));
    }
    rows
}

#[must_use]
pub fn screen_width(columns: usize) -> usize {
    columns.clamp(MIN_WIDTH, MAX_WIDTH)
}

/// Render a stable frame. On a short terminal, links fold before the title and
/// key footer are allowed to scroll away.
#[must_use]
pub fn render_screen(
    view: &ScreenView,
    columns: usize,
    colors: Colors,
    rows: usize,
) -> Vec<String> {
    let width = screen_width(columns);
    let inner = width - 4;
    let statement = truncate(
        &format!(
            "\"{}\"",
            if view.statement.is_empty() {
                "(empty)"
            } else {
                &view.statement
            }
        ),
        inner,
    );
    let edges = edge_rows(view);
    let mut rest = vec![Vec::new()];
    rest.extend(guidance_rows(view, inner));
    rest.extend(question_rows(view, inner));
    rest.push(Vec::new());
    let mut content = vec![
        header(view, inner),
        vec![Segment::plain(statement)],
        Vec::new(),
    ];
    content.extend(edges);
    content.extend(rest);
    let mut footer = Vec::new();
    if let Some(notice) = &view.notice {
        footer.push(vec![Segment::styled(format!("✓ {notice}"), Style::Green)]);
    }
    footer.extend(key_rows(view, inner));
    let title = truncate(&format!(" luuna-roots think {} ", view.slug), width - 4);
    let top = format!(
        "┌─{title}{}┐",
        "─".repeat(width.saturating_sub(3 + title.chars().count()))
    );
    let hint = wrap_text(&view.editor_hint, width - 2)
        .into_iter()
        .map(|line| format!(" {}", colors.dim(&line)))
        .collect::<Vec<_>>();
    let room = if rows > 0 {
        rows.saturating_sub(footer.len() + 2 + hint.len())
    } else {
        usize::MAX
    };
    if content.len() > room && view.edges.len() > 1 {
        content = vec![
            header(view, inner),
            vec![Segment::plain(truncate(
                &format!("\"{}\"", view.statement),
                inner,
            ))],
            Vec::new(),
            vec![Segment::styled(
                format!("{} links (terminal too short)", view.edges.len()),
                Style::Dim,
            )],
        ];
        let mut remaining = vec![Vec::new()];
        remaining.extend(guidance_rows(view, inner));
        remaining.extend(question_rows(view, inner));
        remaining.push(Vec::new());
        content.extend(remaining);
    }
    content.truncate(room);
    let spare = if rows > 0 {
        rows.saturating_sub(content.len() + footer.len() + 2 + hint.len())
    } else {
        0
    };
    content.extend(std::iter::repeat_with(Vec::new).take(spare));
    content.extend(footer);
    let mut output = Vec::with_capacity(content.len() + 2 + hint.len());
    output.push(colors.dim(&top));
    output.extend(
        content
            .iter()
            .map(|row| format!("│ {} │", fit(row, inner, colors))),
    );
    output.push(colors.dim(&format!("└{}┘", "─".repeat(width - 2))));
    output.extend(hint);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use roots_core::color::{Colors, visible_length};

    fn view() -> ScreenView {
        ScreenView {
            id: "r-a1b2".into(),
            slug: "offline-sync".into(),
            status: "shaping".into(),
            statement: "Sync works fully offline, and nobody loses work when the network drops."
                .into(),
            edges: vec![
                ScreenEdge {
                    rel: EdgeRel::Serves,
                    outgoing: true,
                    id: "r-c3d4".into(),
                    slug: "local-first".into(),
                },
                ScreenEdge {
                    rel: EdgeRel::Tension,
                    outgoing: false,
                    id: "r-e5f6".into(),
                    slug: "server-authoritative".into(),
                },
            ],
            question: Some(ScreenQuestion {
                id: "q-7f3a".into(),
                text: "What happens to an offline edit on a record someone else deleted?".into(),
                finding: None,
                source: "agent".into(),
                index: 2,
                total: 3,
            }),
            notice: Some("answered (lines 3–5)".into()),
            editor_hint: "Open idea.md in another pane.".into(),
            can_delegate: false,
            guidance: None,
        }
    }

    #[test]
    fn screen_keeps_footer_and_borders_at_requested_height() {
        let lines = render_screen(&view(), 60, Colors::new(false), 30);
        assert_eq!(lines.len(), 30);
        assert!(lines.iter().any(|line| line.contains("[q] end session")));
        let colored = render_screen(&view(), 60, Colors::new(true), 0);
        let question_box = colored
            .iter()
            .take_while(|line| !line.contains('└'))
            .collect::<Vec<_>>();
        assert!(question_box.iter().all(|line| visible_length(line) == 60));
    }

    #[test]
    fn wrapping_and_findings_keep_human_visible_context() {
        assert_eq!(wrap_text("a bb ccc", 4), ["a bb", "ccc"]);
        let mut screen = view();
        screen.can_delegate = true;
        screen.question.as_mut().unwrap().finding = Some(ScreenFinding {
            text: "Postgres raises unless ON CONFLICT is given.".into(),
            path: "n/f.md".into(),
        });
        let output = render_screen(&screen, 60, Colors::new(false), 0).join("\n");
        assert!(output.contains("── findings ── [agent]"));
        assert!(output.contains("[a] ask agent"));
    }
}
