//! Native chart author/edit (D4-gap — doc 63 §3). Reads a chart part
//! (`xl/charts/chartN.xml`) — the `c:ser` series model (name + category ref +
//! values ref). Authoring edits the same part (chart series, axis titles);
//! the read half is the self-contained, testable foundation the univer chart
//! model was compared against.

use roxmltree::Document;
use thiserror::Error;

/// DrawingML chart namespace (`c:` in chart parts).
const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartSeries {
    /// 0-based series index.
    pub index: usize,
    /// Series name (from `c:tx`), empty when unnamed.
    pub name: String,
    /// The category range reference (`c:cat` → `c:f`), e.g. `Sheet1!$A$2:$A$5`.
    pub categories_ref: Option<String>,
    /// The value range reference (`c:val` → `c:f`).
    pub values_ref: Option<String>,
}

#[derive(Debug, Error)]
pub enum ChartError {
    #[error("xml parse error: {0}")]
    Parse(#[from] roxmltree::Error),
    #[error("no chart space in part")]
    NoChart,
}

/// The first `c:v` text under a node (used for the series name `c:tx`).
fn first_v_text(node: roxmltree::Node) -> Option<String> {
    node.descendants()
        .find(|d| {
            d.is_element() && d.tag_name().namespace() == Some(C) && d.tag_name().name() == "v"
        })
        .and_then(|v| v.text())
        .map(str::to_string)
}

/// The `c:f` formula text under a node (category/value range reference).
fn f_text(node: roxmltree::Node) -> Option<String> {
    node.descendants()
        .find(|d| {
            d.is_element() && d.tag_name().namespace() == Some(C) && d.tag_name().name() == "f"
        })
        .and_then(|v| v.text())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// Chart authoring (the D4-gap "author/edit" half — doc 63 §3)
// ---------------------------------------------------------------------------

/// The chart type the authoring helper emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Pie,
}

impl ChartKind {
    /// The `c:chart` element name (`c:barChart` / `c:lineChart` / `c:pieChart`).
    fn element(self) -> &'static str {
        match self {
            ChartKind::Bar => "barChart",
            ChartKind::Line => "lineChart",
            ChartKind::Pie => "pieChart",
        }
    }
}

/// A series ready to author into a chart part.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartSeriesSpec {
    /// Series name (becomes `c:tx`).
    pub name: String,
    /// Category range ref, e.g. `Sheet1!$A$2:$A$5`.
    pub categories_ref: String,
    /// Value range ref, e.g. `Sheet1!$B$2:$B$5`.
    pub values_ref: String,
}

impl ChartSeriesSpec {
    pub fn new(
        name: impl Into<String>,
        categories_ref: impl Into<String>,
        values_ref: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            categories_ref: categories_ref.into(),
            values_ref: values_ref.into(),
        }
    }
}

/// Escape XML text content for chart values (series names / axis titles).
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Build a `xl/charts/chartN.xml` part for `series` with a title. This is the
/// authoring half of the D4-gap: the caller registers the part (rels + content
/// type via [`chart_rel_fragment`]/[`chart_content_type_override`]) and
/// points a `c:chart` reference at it from the drawing.
pub fn build_chart_part(kind: ChartKind, title: &str, series: &[ChartSeriesSpec]) -> String {
    let mut ser_xml = String::new();
    for (i, s) in series.iter().enumerate() {
        ser_xml.push_str(&format!(
            "    <c:ser>\n      <c:idx val=\"{i}\"/>\n      <c:order val=\"{i}\"/>\n      <c:tx><c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"1\"/><c:pt idx=\"0\"><c:v>{}</c:v></c:pt></c:strCache></c:strRef></c:tx>\n      <c:cat><c:strRef><c:f>{}</c:f></c:strRef></c:cat>\n      <c:val><c:numRef><c:f>{}</c:f></c:numRef></c:val>\n    </c:ser>\n",
            esc(&s.name),
            esc(&s.name),
            esc(&s.categories_ref),
            esc(&s.values_ref),
        ));
    }
    format!(
        r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
<c:lang val="en-US"/>
<c:chart>
  <c:title><c:tx><c:rich><a:bodyPr/><a:p><a:r><a:rPr lang="en-US"/><a:t>{title}</a:t></a:r></a:p></c:rich></c:tx></c:title>
  <c:plotArea><c:layout/><c:{kind}>
{series}<c:axId val="1001"/><c:axId val="1002"/>
  </c:{kind}></c:plotArea>
</c:chart>
</c:chartSpace>"#,
        title = esc(title),
        kind = kind.element(),
        series = ser_xml,
    )
}

/// The `<Relationship>` fragment to append to the sheet's rels part when a
/// new chart part is registered (Type = chart relationship, Target = the
/// chart part relative to the sheet).
pub fn chart_rel_fragment(rel_id: &str, chart_part: &str) -> String {
    format!(
        r#"<Relationship Id="{rel_id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="{chart_part}"/>{newline}"#,
        rel_id = rel_id,
        chart_part = chart_part,
        newline = ""
    )
}

/// The `<Override>` fragment to append to `[Content_Types].xml` when a new
/// chart part is registered.
pub fn chart_content_type_override(chart_part: &str) -> String {
    format!(
        "<Override PartName=\"/{chart_part}\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chart+xml\"/>"
    )
}

/// Extract every chart series from a chart part.
pub fn extract_chart_series(chart_xml: &str) -> Result<Vec<ChartSeries>, ChartError> {
    let doc = Document::parse(chart_xml)?;
    let mut out = Vec::new();
    for (index, ser) in doc
        .descendants()
        .filter(|d| {
            d.is_element() && d.tag_name().namespace() == Some(C) && d.tag_name().name() == "ser"
        })
        .enumerate()
    {
        let name = ser
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "tx")
            .and_then(first_v_text)
            .unwrap_or_default();
        let categories_ref = ser
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "cat")
            .and_then(f_text);
        let values_ref = ser
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "val")
            .and_then(f_text);
        out.push(ChartSeries {
            index,
            name,
            categories_ref,
            values_ref,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
<c:chart><c:plotArea><c:barChart>
<c:ser>
  <c:tx><c:strRef><c:f>Sheet1!$B$1</c:f><c:strCache><c:pt idx="0"><c:v>Sales</c:v></c:pt></c:strCache></c:strRef></c:tx>
  <c:cat><c:strRef><c:f>Sheet1!$A$2:$A$5</c:f></c:strRef></c:cat>
  <c:val><c:numRef><c:f>Sheet1!$B$2:$B$5</c:f></c:numRef></c:val>
</c:ser>
</c:barChart></c:plotArea></c:chart></c:chartSpace>"#;

    #[test]
    fn extracts_series_model() {
        let series = extract_chart_series(CHART).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].index, 0);
        assert_eq!(series[0].name, "Sales");
        assert_eq!(
            series[0].categories_ref.as_deref(),
            Some("Sheet1!$A$2:$A$5")
        );
        assert_eq!(series[0].values_ref.as_deref(), Some("Sheet1!$B$2:$B$5"));
    }

    #[test]
    fn empty_chart_has_no_series() {
        let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart><c:plotArea/></c:chart></c:chartSpace>"#;
        assert!(extract_chart_series(xml).unwrap().is_empty());
    }

    #[test]
    fn builds_bar_chart_part_with_series() {
        let xml = build_chart_part(
            ChartKind::Bar,
            "Sales by quarter",
            &[ChartSeriesSpec::new(
                "Sales",
                "Sheet1!$A$2:$A$5",
                "Sheet1!$B$2:$B$5",
            )],
        );
        assert!(xml.contains("<c:barChart>"), "{xml}");
        assert!(xml.contains("<c:v>Sales</c:v>"), "{xml}");
        assert!(xml.contains("Sheet1!$A$2:$A$5"));
        assert!(xml.contains("Sheet1!$B$2:$B$5"));
        assert!(xml.contains("Sales by quarter"));
        // The authored part re-parses and reads back its series.
        let series = extract_chart_series(&xml).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "Sales");
        assert_eq!(
            series[0].categories_ref.as_deref(),
            Some("Sheet1!$A$2:$A$5")
        );
        assert_eq!(series[0].values_ref.as_deref(), Some("Sheet1!$B$2:$B$5"));
    }

    #[test]
    fn builds_line_and_pie_charts() {
        let line = build_chart_part(ChartKind::Line, "Trend", &[]);
        assert!(line.contains("<c:lineChart>"), "{line}");
        let pie = build_chart_part(ChartKind::Pie, "Share", &[]);
        assert!(pie.contains("<c:pieChart>"), "{pie}");
    }

    #[test]
    fn escapes_series_names() {
        let xml = build_chart_part(
            ChartKind::Bar,
            "Q1 & Q2",
            &[ChartSeriesSpec::new("A & B < C", "S!A1:A2", "S!B1:B2")],
        );
        assert!(xml.contains("Q1 &amp; Q2"));
        assert!(xml.contains("A &amp; B &lt; C"));
    }

    #[test]
    fn registration_fragments_are_wellformed() {
        let rel = chart_rel_fragment("rId10", "../charts/chart1.xml");
        assert!(rel.contains("Id=\"rId10\""));
        assert!(rel.contains(
            "Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart\""
        ));
        assert!(rel.contains("Target=\"../charts/chart1.xml\""));
        let ovr = chart_content_type_override("xl/charts/chart1.xml");
        assert!(ovr.contains("PartName=\"/xl/charts/chart1.xml\""));
        assert!(ovr.contains("drawingml.chart+xml"));
    }

    #[test]
    fn unnamed_series_is_empty_string() {
        let xml = r#"<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart">
<c:chart><c:plotArea><c:barChart><c:ser><c:cat><c:strRef><c:f>S!A1:A2</c:f></c:strRef></c:cat><c:val><c:numRef><c:f>S!B1:B2</c:f></c:numRef></c:val></c:ser></c:barChart></c:plotArea></c:chart></c:chartSpace>"#;
        let series = extract_chart_series(xml).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "");
        assert_eq!(series[0].values_ref.as_deref(), Some("S!B1:B2"));
    }
}
