use std::collections::{HashMap, HashSet};

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::*;

pub(super) struct AgentRow {
    pub(super) pane_id: String,
    pub(super) workspace_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) focused: bool,
    pub(super) rows: Vec<Vec<crate::ui::ResolvedToken>>,
    pub(super) group_color: Option<crate::config::SidebarTokenColor>,
}

pub(super) struct AgentGroupRow {
    pub(super) workspace_id: String,
    pub(super) status: crate::api::schema::AgentStatus,
    pub(super) count: usize,
    pub(super) collapsed: bool,
}

pub(super) enum AgentPanelItem {
    Group(AgentGroupRow),
    Agent(AgentRow),
}

pub(super) fn ordered_agent_pane_ids(
    snapshot: &ClientShellSnapshot,
    sort: crate::config::AgentPanelSortConfig,
) -> Vec<String> {
    if snapshot.agent_view_label.is_some() {
        return snapshot
            .agent_order
            .iter()
            .filter(|pane_id| {
                snapshot
                    .agents
                    .iter()
                    .any(|agent| agent.pane_id == pane_id.as_str())
            })
            .cloned()
            .collect();
    }
    let mut agents = snapshot.agents.iter().collect::<Vec<_>>();
    if sort == crate::config::AgentPanelSortConfig::Priority {
        agents.sort_by_key(|agent| {
            (
                std::cmp::Reverse(status_priority(agent.agent_status)),
                std::cmp::Reverse(agent.state_change_seq),
            )
        });
    }
    agents
        .into_iter()
        .map(|agent| agent.pane_id.clone())
        .collect()
}

pub(super) fn render_agent_panel(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
) {
    if !render_agent_panel_header(
        buffer,
        area,
        snapshot.agent_view_label.as_deref(),
        config,
        hits,
    ) {
        return;
    }

    let items = agent_panel_items(snapshot, config, collapsed_agent_groups);
    let nested = u16::from(
        items
            .iter()
            .any(|item| matches!(item, AgentPanelItem::Group(_))),
    ) * 2;
    render_agent_list(
        buffer,
        area,
        &items,
        snapshot
            .agent_view_label
            .as_ref()
            .map(|_| " no matching agents"),
        config,
        agent_scroll,
        hits,
        |item| match item {
            AgentPanelItem::Group(_) => 1,
            AgentPanelItem::Agent(row) => row.rows.len(),
        },
        |buffer, rect, item, hits| match item {
            AgentPanelItem::Group(group) => {
                hits.agent_group_toggles
                    .push((rect, group.workspace_id.clone()));
                render_agent_group_row(buffer, rect, group, snapshot, config);
            }
            AgentPanelItem::Agent(row) => {
                hits.agents.push((rect, row.pane_id.clone()));
                render_agent_row(buffer, rect, row, config, nested);
            }
        },
    );
}

/// Groups agents under a collapsible header per workspace; only the grouped
/// (spaces) ordering has a stable workspace order to group by.
pub(super) fn agent_panel_items(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    collapsed_agent_groups: &HashSet<String>,
) -> Vec<AgentPanelItem> {
    let rows = agent_rows(snapshot, config, None);
    if snapshot.agent_view_label.is_some()
        || config.agent_panel_sort != crate::config::AgentPanelSortConfig::Spaces
    {
        return rows.into_iter().map(AgentPanelItem::Agent).collect();
    }
    let mut groups = Vec::<(String, Vec<AgentRow>)>::new();
    for row in rows {
        match groups.iter_mut().find(|(id, _)| *id == row.workspace_id) {
            Some((_, members)) => members.push(row),
            None => groups.push((row.workspace_id.clone(), vec![row])),
        }
    }
    let mut items = Vec::new();
    for (workspace_id, members) in groups {
        let collapsed = collapsed_agent_groups.contains(&workspace_id);
        let status = members
            .iter()
            .map(|row| row.status)
            .max_by_key(|status| status_priority(*status))
            .unwrap_or(crate::api::schema::AgentStatus::Unknown);
        let color = workspace_color(snapshot, config, &workspace_id, status);
        items.push(AgentPanelItem::Group(AgentGroupRow {
            status,
            count: members.len(),
            collapsed,
            workspace_id,
        }));
        if !collapsed {
            items.extend(members.into_iter().map(|mut row| {
                row.group_color = color;
                AgentPanelItem::Agent(row)
            }));
        }
    }
    items
}

/// The workspace's color as resolved by the Spaces row rules, so a group
/// matches its Space entry.
fn workspace_color(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    workspace_id: &str,
    status: crate::api::schema::AgentStatus,
) -> Option<crate::config::SidebarTokenColor> {
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == workspace_id)?;
    super::sidebar::workspace_rows(snapshot, workspace, status, false, &config.spaces)
        .into_iter()
        .flatten()
        .find(|token| matches!(token.kind, crate::ui::ResolvedTokenKind::Workspace(_)))?
        .style
        .fg
}

fn render_agent_group_row(
    buffer: &mut Buffer,
    rect: Rect,
    group: &AgentGroupRow,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    put_text(
        buffer,
        rect.x.saturating_add(1),
        rect.y,
        rect.width.saturating_sub(1),
        if group.collapsed { "▸" } else { "▾" },
        Style::default().fg(palette.accent),
    );
    let Some(workspace) = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == group.workspace_id)
    else {
        return;
    };
    let tokens =
        super::sidebar::workspace_rows(snapshot, workspace, group.status, false, &config.spaces)
            .into_iter()
            .next()
            .unwrap_or_default();
    let count = format!(" {}", group.count);
    let x = rect.x.saturating_add(3);
    let width = rect.right().saturating_sub(x);
    let status_style = Style::default().fg(status_color(group.status, palette));
    let mut spans = crate::ui::resolved_token_spans(
        &tokens,
        (
            status_icon(group.status, config.status_indicators),
            status_style,
        ),
        status_style,
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD),
        Style::default().fg(palette.overlay0),
        Style::default().fg(palette.overlay1),
        palette,
        (width as usize).saturating_sub(display_width(&count)),
    );
    spans.push(Span::styled(count, Style::default().fg(palette.overlay0)));
    Paragraph::new(Line::from(spans)).render(Rect::new(x, rect.y, width, 1), buffer);
}

pub(super) fn render_agent_panel_header(
    buffer: &mut Buffer,
    area: Rect,
    agent_view_label: Option<&str>,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) -> bool {
    if area.height == 0 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y,
        area.width,
        &"─".repeat(area.width as usize),
        Style::default().fg(config.palette.surface_dim),
    );
    if area.height < 2 {
        return false;
    }
    put_text(
        buffer,
        area.x,
        area.y + 1,
        area.width,
        " agents",
        Style::default()
            .fg(config.palette.overlay0)
            .add_modifier(Modifier::BOLD),
    );
    let sort_label = agent_view_label.unwrap_or(match config.agent_panel_sort {
        crate::config::AgentPanelSortConfig::Spaces => "grouped",
        crate::config::AgentPanelSortConfig::Priority => "priority",
    });
    let sort_width = display_width(sort_label).min(area.width as usize) as u16;
    let sort_rect = Rect::new(
        area.right().saturating_sub(sort_width),
        area.y + 1,
        sort_width,
        1,
    );
    hits.agent_sort_toggle = if config.mouse_capture && agent_view_label.is_none() {
        sort_rect
    } else {
        Rect::default()
    };
    put_text(
        buffer,
        sort_rect.x,
        sort_rect.y,
        sort_rect.width,
        sort_label,
        Style::default()
            .fg(if agent_view_label.is_some() {
                config.palette.accent
            } else {
                config.palette.overlay0
            })
            .add_modifier(Modifier::BOLD),
    );
    true
}

pub(super) fn render_agent_list<T>(
    buffer: &mut Buffer,
    area: Rect,
    rows: &[T],
    empty_message: Option<&str>,
    config: &ClientShellConfig,
    agent_scroll: &mut usize,
    hits: &mut ShellHitMap,
    row_lines: impl Fn(&T) -> usize,
    mut render_row: impl FnMut(&mut Buffer, Rect, &T, &mut ShellHitMap),
) {
    let body = Rect::new(
        area.x,
        area.y.saturating_add(3),
        area.width,
        area.height.saturating_sub(3),
    );
    hits.agent_body = body;
    if body.is_empty() || rows.is_empty() {
        *agent_scroll = 0;
        if let Some(message) = empty_message.filter(|_| !body.is_empty()) {
            put_text(
                buffer,
                body.x,
                body.y,
                body.width,
                message,
                Style::default()
                    .fg(config.palette.overlay0)
                    .add_modifier(Modifier::DIM),
            );
        }
        return;
    }

    let row_heights = rows
        .iter()
        .map(|row| row_lines(row).max(1).min(u16::MAX as usize) as u16)
        .collect::<Vec<_>>();
    let gaps = rows
        .iter()
        .enumerate()
        .map(|(index, _)| {
            if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            }
        })
        .collect::<Vec<_>>();
    let metrics =
        super::scroll::list_scroll_metrics(&row_heights, &gaps, body.height, *agent_scroll);
    hits.agent_max_scroll = metrics.max_offset_from_bottom;
    hits.agent_scroll_metrics = Some(metrics);
    *agent_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let content_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*agent_scroll) {
        let height = row_heights[index].min(body.height);
        if y.saturating_add(height) > body.bottom() {
            break;
        }
        let rect = Rect::new(body.x, y, content_width, height);
        render_row(buffer, rect, row, hits);
        y = y
            .saturating_add(height)
            .saturating_add(if index + 1 < rows.len() {
                config.agents.row_gap
            } else {
                0
            });
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.agent_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, &config.palette);
    }
}

pub(super) fn agent_rows(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Vec<AgentRow> {
    ordered_agent_pane_ids(snapshot, config.agent_panel_sort)
        .into_iter()
        .filter_map(|pane_id| agent_row(snapshot, &pane_id, config, machine))
        .collect()
}

pub(super) fn agent_row(
    snapshot: &ClientShellSnapshot,
    pane_id: &str,
    config: &ClientShellConfig,
    machine: Option<&str>,
) -> Option<AgentRow> {
    let agent = snapshot
        .agents
        .iter()
        .find(|agent| agent.pane_id == pane_id)?;
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == agent.workspace_id)?;
    let tab = snapshot.tabs.iter().find(|tab| tab.tab_id == agent.tab_id);
    let pane = snapshot
        .panes
        .iter()
        .find(|pane| pane.pane_id == agent.pane_id);
    let tab_count = snapshot
        .tabs
        .iter()
        .filter(|candidate| candidate.workspace_id == agent.workspace_id)
        .count();
    let tab_label = tab
        .filter(|tab| tab_count > 1 || tab.custom_label)
        .map(|tab| tab.label.as_str());
    let agent_label = agent
        .display_agent
        .as_deref()
        .or(agent.name.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref());
    let labels = agent
        .state_labels
        .iter()
        .cloned()
        .collect::<HashMap<_, _>>();
    let tokens = agent.tokens.iter().cloned().collect::<HashMap<_, _>>();
    let state_text = labels
        .get(status_text(agent.agent_status))
        .map(String::as_str)
        .unwrap_or_else(|| sidebar_status_text(agent.agent_status));
    let canonical_agent = agent
        .agent
        .as_deref()
        .and_then(crate::detect::parse_agent_label);
    let rows = crate::ui::sidebar_agent_rows(
        &config.agents,
        crate::ui::AgentTokenContext {
            machine,
            workspace: &workspace.label,
            tab: tab_label,
            pane: agent
                .title
                .as_deref()
                .or_else(|| pane.and_then(|pane| pane.label.as_deref())),
            agent_label,
            terminal_title: agent.terminal_title.as_deref(),
            terminal_title_stripped: agent.terminal_title_stripped.as_deref(),
            canonical_agent,
            tokens: &tokens,
        },
        state_text,
    );
    Some(AgentRow {
        pane_id: agent.pane_id.clone(),
        workspace_id: agent.workspace_id.clone(),
        group_color: None,
        status: agent.agent_status,
        focused: agent.focused,
        rows,
    })
}

pub(super) fn render_agent_row(
    buffer: &mut Buffer,
    rect: Rect,
    row: &AgentRow,
    config: &ClientShellConfig,
    nested: u16,
) {
    let palette = &config.palette;
    let row_style = if row.focused {
        Style::default().bg(palette.active_row_bg)
    } else {
        Style::default()
    };
    let name_style = if row.focused {
        Style::default()
            .fg(palette.text)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(palette.subtext0)
            .add_modifier(Modifier::BOLD)
    };
    let status_style = Style::default().fg(status_color(row.status, palette));
    let secondary = Style::default().fg(palette.overlay0);
    let icon = (
        status_icon(row.status, config.status_indicators),
        Style::default().fg(status_color(row.status, palette)),
    );
    let rows = if row.rows.is_empty() {
        vec![vec![crate::ui::ResolvedToken {
            kind: crate::ui::ResolvedTokenKind::StateIcon,
            style: Default::default(),
        }]]
    } else {
        row.rows.clone()
    };
    let muted = row
        .group_color
        .map(|color| muted_color(color, palette.overlay0));
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(index, tokens)| {
            tinted_tokens(tokens, if index == 0 { row.group_color } else { muted })
        })
        .collect::<Vec<_>>();
    for (index, tokens) in rows.iter().take(rect.height as usize).enumerate() {
        let indent = usize::from(nested) + if index == 0 { 1 } else { 3 };
        let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent))];
        spans.extend(crate::ui::resolved_token_spans(
            tokens,
            icon,
            status_style,
            name_style,
            secondary,
            secondary,
            palette,
            rect.width.saturating_sub(indent as u16) as usize,
        ));
        Paragraph::new(Line::from(spans)).style(row_style).render(
            Rect::new(rect.x, rect.y + index as u16, rect.width, 1),
            buffer,
        );
    }
}

/// Colors a grouped agent's text like its group header, one step weaker (never
/// bold). Explicit token colors and the state icon keep their own color.
fn tinted_tokens(
    tokens: Vec<crate::ui::ResolvedToken>,
    color: Option<crate::config::SidebarTokenColor>,
) -> Vec<crate::ui::ResolvedToken> {
    let Some(color) = color else {
        return tokens;
    };
    tokens
        .into_iter()
        .map(|mut token| {
            if !matches!(token.kind, crate::ui::ResolvedTokenKind::StateIcon) {
                token.style.fg.get_or_insert(color);
                token.style.bold.get_or_insert(false);
            }
            token
        })
        .collect()
}

/// Mutes by blending toward the muted-text color instead of terminal faint,
/// which the sidebar avoids so muted rows render the same on every terminal.
fn muted_color(
    color: crate::config::SidebarTokenColor,
    toward: ratatui::style::Color,
) -> crate::config::SidebarTokenColor {
    let ratatui::style::Color::Rgb(r, g, b) = color.ratatui() else {
        return color;
    };
    let (tr, tg, tb) = match toward {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    };
    let mix = |a: u8, b: u8| ((u16::from(a) + u16::from(b)) / 2) as u8;
    crate::config::SidebarTokenColor::rgb(mix(r, tr), mix(g, tg), mix(b, tb))
}

fn put_text(buffer: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    for (offset, character) in text.chars().take(width as usize).enumerate() {
        if let Some(cell) = buffer.cell_mut((x + offset as u16, y)) {
            cell.set_char(character).set_style(style);
        }
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn sidebar_status_text(status: crate::api::schema::AgentStatus) -> &'static str {
    use crate::api::schema::AgentStatus;
    match status {
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Working => "working",
        AgentStatus::Idle | AgentStatus::Unknown => "idle",
    }
}
