//! Drawing the Meshtastic pulse: one row per node, its names on the left,
//! a tick for each packet heard from it in the middle — all rows on one
//! time axis of the wall clock, now at the right — and on the right how
//! long ago it was heard, with the SNR, RSSI and hops of its last packet.
//! The axis and the columns' names are on top; under the rows, what the
//! list shows when not all, how to start `meshtasticd` when it does not
//! answer, and the selected node with its last packet.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use sdr_mesh::update::{BROADCAST, Content, Packet};

use crate::channels::format_ago;
use crate::mesh::{ACTIVE_S, Link, MeshView, NodeRow, Order};
use crate::spectrum::BARS;

/// Columns for the names: "HG1  Haifa Gate" and a space.
const LABEL: u16 = 22;

/// Columns right of the ticks: "  16 m  -13.2  -115    2".
const FIGURES: u16 = 25;

const HEARD: Color = Color::Rgb(90, 200, 120);
const OWN: Color = Color::Cyan;

pub(crate) fn draw(frame: &mut Frame, view: &MeshView, area: Rect) {
    let rows = view.rows();
    let notes: Vec<Line> = [start_hint(view), shown_line(view, rows.len())]
        .into_iter()
        .flatten()
        .collect();
    let card = if view.selected().is_some() { 2 } else { 0 };
    let [header, axis, list, notes_area, card_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(notes.len() as u16),
        Constraint::Length(card),
    ])
    .areas(area);
    let columns = usize::from(area.width - LABEL - FIGURES);
    frame.render_widget(header_line(view), header);
    frame.render_widget(Paragraph::new(notes), notes_area);
    if rows.is_empty() {
        let text = Paragraph::new(empty_text(&view.link))
            .centered()
            .wrap(Wrap { trim: true })
            .dark_gray();
        frame.render_widget(text, list);
        return;
    }
    draw_axis(frame, view, columns, axis);
    // The selection is kept in view, in the middle when it can be.
    let height = usize::from(list.height);
    let selected = view.selected_row();
    let first = selected
        .map_or(0, |row| row.saturating_sub(height / 2))
        .min(rows.len().saturating_sub(height));
    let listened = view.listened(columns);
    for (y, row) in rows.iter().enumerate().skip(first).take(height) {
        let area = Rect {
            y: list.y + (y - first) as u16,
            height: 1,
            ..list
        };
        let ticks = Ticks {
            cells: view.cells(row, columns),
            listened: &listened,
        };
        draw_row(frame, view, row, selected == Some(y), &ticks, area);
    }
    if let Some(row) = view.selected() {
        draw_card(frame, view, row, card_area);
    }
}

/// A row's packets per column, and the columns the app was listening in.
struct Ticks<'a> {
    cells: Vec<u32>,
    listened: &'a [bool],
}

fn empty_text(link: &Link) -> String {
    match link {
        Link::Off => "Started without --mesh: `sdr-viewer --mesh` listens to the meshtasticd \
                      of this machine, `--mesh=HOST` to another's."
            .into(),
        Link::Connecting(addr) => format!("Connecting to meshtasticd at {addr}…"),
        Link::Connected => "meshtasticd knows no nodes yet.".into(),
        Link::Lost(why) => format!("No answer from meshtasticd: {why}."),
    }
}

/// While `meshtasticd` does not answer: how to start it.
fn start_hint(view: &MeshView) -> Option<Line<'static>> {
    if !matches!(view.link, Link::Lost(_)) {
        return None;
    }
    let line = match &view.starter {
        Some(command) => Line::from(vec![
            Span::raw(" Is meshtasticd running? Start it: ").yellow(),
            Span::raw(command.clone()).bold(),
            Span::raw(" — the pulse connects by itself.").yellow(),
        ]),
        None => {
            Line::from(" Is meshtasticd running there? The pulse connects by itself once it is.")
                .yellow()
        }
    };
    Some(line)
}

/// What the list shows, when it is not all the nodes the newest first, and
/// the keys that change it.
fn shown_line(view: &MeshView, shown: usize) -> Option<Line<'static>> {
    let mut heard = Vec::new();
    if view.active_only() {
        heard.push(format!("in the last {} m (a)", ACTIVE_S / 60));
    }
    if let Some(since) = view.since() {
        heard.push(format!("since {} (m)", view.time_of_day(since)));
    }
    let mut parts = Vec::new();
    if !heard.is_empty() {
        parts.push(format!(
            "{shown} of {} nodes, heard {}",
            view.known(),
            heard.join(" and ")
        ));
    }
    if view.order() == Order::Hops {
        parts.push("the fewest hops first (s)".to_string());
    }
    (!parts.is_empty()).then(|| Line::from(format!(" {}", parts.join(" · "))).dark_gray())
}

/// The node itself and its radio, or what is wrong with the connection,
/// and how many nodes there are.
fn header_line(view: &MeshView) -> Line<'static> {
    let dim = |text: String| Span::raw(text).dark_gray();
    let mut spans = vec![Span::raw(" ")];
    match &view.link {
        Link::Connected => {}
        Link::Off => spans.push(Span::raw("no meshtasticd").yellow()),
        Link::Connecting(addr) => spans.push(Span::raw(format!("connecting to {addr}…")).yellow()),
        Link::Lost(why) => spans.push(Span::raw(format!("lost: {why}")).red()),
    }
    if let Some(ready) = &view.ready {
        if spans.len() > 1 {
            spans.push(dim(" · ".into()));
        }
        let me = view.rows().into_iter().find(|row| row.num == ready.me);
        let name = me
            .and_then(|row| row.user.as_ref())
            .map(|user| user.long_name.clone());
        spans.push(Span::raw(name.unwrap_or_else(|| format!("!{:08x}", ready.me))).bold());
        if let Some(radio) = &ready.radio {
            if let Some(mhz) = radio.frequency_mhz {
                spans.push(dim(" · ".into()));
                spans.push(Span::raw(format!("{mhz:.3} MHz")));
            }
            let slot = radio
                .slot
                .map_or(String::new(), |slot| format!(" slot {slot}"));
            spans.push(dim(format!(" {} {}{slot}", radio.region, radio.preset)));
        }
    }
    let nodes = view.known();
    spans.extend([
        dim(" · ".into()),
        Span::raw(nodes.to_string()).bold(),
        dim(format!(" node{}, ", if nodes == 1 { "" } else { "s" })),
        Span::raw(view.heard().to_string()).bold(),
        dim(" heard".into()),
    ]);
    Line::from(spans)
}

fn draw_row(
    frame: &mut Frame,
    view: &MeshView,
    row: &NodeRow,
    selected: bool,
    ticks: &Ticks,
    area: Rect,
) {
    let own = view.ready.as_ref().is_some_and(|ready| ready.me == row.num);
    let mut style = match (own, row.packets) {
        (true, _) => Style::new().fg(OWN).add_modifier(Modifier::BOLD),
        (false, 0) => Style::new().fg(Color::DarkGray),
        (false, _) => Style::new(),
    };
    if selected {
        style = style.add_modifier(Modifier::REVERSED);
    }
    let width = usize::from(LABEL) - 1;
    let label = match &row.user {
        Some(user) => format!("{:<4} {}", user.short_name, user.long_name),
        None => format!("{:<4} {}", "", row.id()),
    };
    let label: String = label.chars().take(width).collect();
    frame.render_widget(Span::styled(format!("{label:<width$}"), style), area);
    let ago = row.last_heard.map_or("—".to_string(), |heard| {
        format_ago(Duration::from_secs(view.now().saturating_sub(heard)))
    });
    let snr = row.snr.map_or("—".to_string(), |snr| format!("{snr:.1}"));
    let rssi = row.rssi.map_or("—".to_string(), |rssi| rssi.to_string());
    let hops = row.hops.map_or("—".to_string(), |hops| hops.to_string());
    let figures = format!(" {ago:>6} {snr:>6} {rssi:>5} {hops:>4}");
    frame.render_widget(
        Line::from(figures).right_aligned(),
        Rect {
            x: area.x + area.width - FIGURES,
            width: FIGURES,
            ..area
        },
    );
    for (column, (&count, &listened)) in ticks.cells.iter().zip(ticks.listened).enumerate() {
        let at = &mut frame.buffer_mut()[(area.x + LABEL + column as u16, area.y)];
        if count > 0 {
            // One packet is half a cell; three or more fill it.
            let eighths = (2 + 2 * count as usize).min(8);
            at.set_symbol(BARS[eighths])
                .set_style(Style::new().fg(HEARD));
        } else if listened {
            at.set_symbol("·")
                .set_style(Style::new().fg(Color::DarkGray));
        }
    }
}

/// The time axis over the ticks — what a column stands for left of it,
/// the clock along it — and the figures' names right of it, the one the
/// nodes are sorted by brighter.
fn draw_axis(frame: &mut Frame, view: &MeshView, columns: usize, area: Rect) {
    let scale = Line::from(vec![
        Span::raw(format!(" {}", view.scale_name())),
        Span::raw(" a column").dark_gray(),
    ]);
    frame.render_widget(scale, area);
    let strip = Rect {
        x: area.x + LABEL,
        width: columns as u16,
        ..area
    };
    frame.render_widget(Line::from("now").dark_gray().right_aligned(), strip);
    // A mark begins at its column, apart from the next and short of "now".
    let mut free = 0;
    for (column, time) in view.marks(columns) {
        if column < free || column + time.len() + 1 > columns.saturating_sub(3) {
            continue;
        }
        let at = Rect {
            x: strip.x + column as u16,
            width: time.len() as u16,
            ..strip
        };
        frame.render_widget(Line::from(time.clone()).dark_gray(), at);
        free = column + time.len() + 1;
    }
    let figures = Rect {
        x: area.x + area.width - FIGURES,
        width: FIGURES,
        ..area
    };
    let sorted = match view.order() {
        Order::Heard => "heard",
        Order::Hops => "hops",
    };
    let name = |name: &'static str, width: usize| {
        let text = format!(" {name:>width$}");
        if name == sorted {
            Span::raw(text).bold()
        } else {
            Span::raw(text).dark_gray()
        }
    };
    let names = Line::from(vec![
        name("heard", 6),
        name("SNR", 6),
        name("RSSI", 5),
        name("hops", 4),
    ]);
    frame.render_widget(names.right_aligned(), figures);
}

/// Who the selected node is, and what its last packet was.
fn draw_card(frame: &mut Frame, view: &MeshView, row: &NodeRow, area: Rect) {
    let dim = |text: String| Span::raw(text).dark_gray();
    let mut who = vec![Span::raw(" ")];
    if let Some(user) = &row.user {
        who.push(Span::raw(format!("{} {}", user.short_name, user.long_name)).bold());
        who.push(dim(format!(
            " · {} · {} · {}",
            user.id, user.hw_model, user.role
        )));
    } else {
        who.push(Span::raw(row.id()).bold());
        who.push(dim(" · has not told who it is".into()));
    }
    let heard = match row.packets {
        0 => " · not heard in this run".to_string(),
        1 => " · 1 packet in this run".to_string(),
        n => format!(" · {n} packets in this run"),
    };
    who.push(dim(heard));
    let last = row.last.as_ref().map_or_else(
        || Line::from(dim(" Its packets will show here.".into())),
        |packet| packet_line(view, packet),
    );
    let [first, second] = Layout::vertical([Constraint::Length(1); 2]).areas(area);
    frame.render_widget(Line::from(who), first);
    frame.render_widget(last, second);
}

fn packet_line(view: &MeshView, packet: &Packet) -> Line<'static> {
    let dim = |text: String| Span::raw(text).dark_gray();
    let kind = match &packet.content {
        Content::Encrypted => "encrypted".to_string(),
        Content::User(_) => "node info".to_string(),
        Content::Other { name, .. } => name.clone(),
    };
    let me = view.ready.as_ref().map(|ready| ready.me);
    let to = match packet.to {
        BROADCAST => "all".to_string(),
        to if Some(to) == me => "this node".to_string(),
        to => view
            .rows()
            .into_iter()
            .find(|row| row.num == to)
            .and_then(|row| row.user.as_ref().map(|user| user.short_name.clone()))
            .unwrap_or_else(|| format!("!{to:08x}")),
    };
    let mut spans = vec![
        dim(" last: ".into()),
        Span::raw(kind).bold(),
        dim(format!(" to {to} · {}", view.channel_name(packet))),
    ];
    if packet.snr != 0.0 || packet.rssi != 0 {
        spans.push(dim(" · SNR ".into()));
        spans.push(Span::raw(format!("{:.2} dB", packet.snr)));
        spans.push(dim(" · RSSI ".into()));
        spans.push(Span::raw(format!("{} dBm", packet.rssi)));
    }
    match packet.hops {
        Some(0) => spans.push(dim(" · direct".into())),
        Some(hops) => {
            spans.push(dim(" · ".into()));
            spans.push(Span::raw(format!("{hops} of {} hops", packet.hop_start)));
            let relay = view.relayed_by(packet.relay).map(|row| {
                row.user
                    .as_ref()
                    .map_or_else(|| row.id(), |user| user.short_name.clone())
            });
            match (relay, packet.relay) {
                (Some(name), _) => spans.push(dim(format!(", last by {name}"))),
                (None, 0) => {}
                (None, byte) => spans.push(dim(format!(", last by …{byte:02x}"))),
            }
        }
        None => {}
    }
    if packet.via_mqtt {
        spans.push(dim(" · via MQTT".into()));
    }
    let ago = Duration::from_secs(view.now().saturating_sub(packet.received));
    spans.push(dim(format!(" · {} ago", format_ago(ago))));
    Line::from(spans)
}
