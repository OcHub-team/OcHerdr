use super::*;

pub(crate) const PANE_TEMPLATE_CARD_WIDTH: f32 = 68.;
pub(crate) const PANE_TEMPLATE_CARD_HEIGHT: f32 = 48.;
pub(crate) const PANE_TEMPLATE_CARD_GAP: f32 = 8.;
pub(crate) const PANE_TEMPLATE_PALETTE_PADDING: f32 = 8.;
pub(crate) const PANE_TEMPLATE_PALETTE_TOP: f32 = 10.;
pub(crate) const PANE_TEMPLATE_CELL_INSET: f32 = 5.;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PaneLayoutTemplate {
    TwoColumns,
    TwoRows,
    ThreeColumns,
    ThreeRows,
    ThreeMainLeft,
    ThreeMainRight,
    FourGrid,
    FourColumns,
    FourRows,
    FourMainLeft,
}

impl PaneLayoutTemplate {
    pub(crate) fn pane_count(self) -> usize {
        match self {
            Self::TwoColumns | Self::TwoRows => 2,
            Self::ThreeColumns | Self::ThreeRows | Self::ThreeMainLeft | Self::ThreeMainRight => 3,
            Self::FourGrid | Self::FourColumns | Self::FourRows | Self::FourMainLeft => 4,
        }
    }

    pub(crate) fn label(self, i18n: I18n) -> &'static str {
        i18n.text(match self {
            Self::TwoColumns => k::TERMINAL_LAYOUT_TWO_COLUMNS,
            Self::TwoRows => k::TERMINAL_LAYOUT_TWO_ROWS,
            Self::ThreeColumns => k::TERMINAL_LAYOUT_THREE_COLUMNS,
            Self::ThreeRows => k::TERMINAL_LAYOUT_THREE_ROWS,
            Self::ThreeMainLeft => k::TERMINAL_LAYOUT_THREE_MAIN_LEFT,
            Self::ThreeMainRight => k::TERMINAL_LAYOUT_THREE_MAIN_RIGHT,
            Self::FourGrid => k::TERMINAL_LAYOUT_FOUR_GRID,
            Self::FourColumns => k::TERMINAL_LAYOUT_FOUR_COLUMNS,
            Self::FourRows => k::TERMINAL_LAYOUT_FOUR_ROWS,
            Self::FourMainLeft => k::TERMINAL_LAYOUT_FOUR_MAIN_LEFT,
        })
    }
}

pub(crate) fn pane_layout_templates(pane_count: usize) -> &'static [PaneLayoutTemplate] {
    use PaneLayoutTemplate as T;
    match pane_count {
        2 => &[T::TwoColumns, T::TwoRows],
        3 => &[
            T::ThreeColumns,
            T::ThreeRows,
            T::ThreeMainLeft,
            T::ThreeMainRight,
        ],
        4 => &[T::FourGrid, T::FourColumns, T::FourRows, T::FourMainLeft],
        _ => &[],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PaneTemplatePlacement {
    pub(crate) template: PaneLayoutTemplate,
    pub(crate) slot: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneTemplateHover {
    pub(crate) placement: PaneTemplatePlacement,
    pub(crate) slot_rect: (f32, f32, f32, f32),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneTemplateCardGeometry {
    pub(crate) template: PaneLayoutTemplate,
    pub(crate) rect: (f32, f32, f32, f32),
    pub(crate) slots: Vec<(f32, f32, f32, f32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneTemplatePaletteGeometry {
    pub(crate) rect: (f32, f32, f32, f32),
    pub(crate) cards: Vec<PaneTemplateCardGeometry>,
}

pub(crate) fn pane_template_palette_geometry(
    surface: (f32, f32, f32, f32),
    pane_count: usize,
) -> Option<PaneTemplatePaletteGeometry> {
    let templates = pane_layout_templates(pane_count);
    if templates.is_empty() {
        return None;
    }
    let cards_width = templates.len() as f32 * PANE_TEMPLATE_CARD_WIDTH
        + templates.len().saturating_sub(1) as f32 * PANE_TEMPLATE_CARD_GAP;
    let width = cards_width + PANE_TEMPLATE_PALETTE_PADDING * 2.;
    let height = PANE_TEMPLATE_CARD_HEIGHT + PANE_TEMPLATE_PALETTE_PADDING * 2.;
    let x = surface.0 + ((surface.2 - width) / 2.).max(0.);
    let y = surface.1 + PANE_TEMPLATE_PALETTE_TOP;
    let cards = templates
        .iter()
        .copied()
        .enumerate()
        .map(|(index, template)| {
            let card_x = x
                + PANE_TEMPLATE_PALETTE_PADDING
                + index as f32 * (PANE_TEMPLATE_CARD_WIDTH + PANE_TEMPLATE_CARD_GAP);
            let rect = (
                card_x,
                y + PANE_TEMPLATE_PALETTE_PADDING,
                PANE_TEMPLATE_CARD_WIDTH,
                PANE_TEMPLATE_CARD_HEIGHT,
            );
            let inner = (
                rect.0 + PANE_TEMPLATE_CELL_INSET,
                rect.1 + PANE_TEMPLATE_CELL_INSET,
                rect.2 - PANE_TEMPLATE_CELL_INSET * 2.,
                rect.3 - PANE_TEMPLATE_CELL_INSET * 2.,
            );
            let slots = pane_template_slot_fractions(template)
                .into_iter()
                .map(|slot| fractions_to_window(inner, slot))
                .collect();
            PaneTemplateCardGeometry {
                template,
                rect,
                slots,
            }
        })
        .collect();
    Some(PaneTemplatePaletteGeometry {
        rect: (x, y, width, height),
        cards,
    })
}

pub(crate) fn pane_template_hover(
    surface: (f32, f32, f32, f32),
    pane_count: usize,
    pointer: (f32, f32),
) -> Option<PaneTemplateHover> {
    let palette = pane_template_palette_geometry(surface, pane_count)?;
    palette.cards.into_iter().find_map(|card| {
        card.slots
            .into_iter()
            .enumerate()
            .find(|(_, rect)| point_in_rect(pointer, *rect))
            .map(|(slot, slot_rect)| PaneTemplateHover {
                placement: PaneTemplatePlacement {
                    template: card.template,
                    slot,
                },
                slot_rect,
            })
    })
}

fn point_in_rect(point: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    point.0 >= rect.0
        && point.0 <= rect.0 + rect.2
        && point.1 >= rect.1
        && point.1 <= rect.1 + rect.3
}

pub(crate) fn pane_template_target_tree(
    layout: &PaneLayout,
    source_pane_id: &str,
    placement: PaneTemplatePlacement,
) -> Option<LayoutNode> {
    if layout.panes.len() != placement.template.pane_count()
        || placement.slot >= layout.panes.len()
        || !layout
            .panes
            .iter()
            .any(|pane| pane.pane_id == source_pane_id)
    {
        return None;
    }
    let mut remaining = layout
        .panes
        .iter()
        .map(|pane| pane.pane_id.clone())
        .filter(|pane_id| pane_id != source_pane_id);
    let ids = (0..layout.panes.len())
        .map(|slot| {
            if slot == placement.slot {
                Some(source_pane_id.to_owned())
            } else {
                remaining.next()
            }
        })
        .collect::<Option<Vec<_>>>()?;
    template_tree(placement.template, &ids)
}

pub(crate) fn pane_template_predicted_layout(
    layout: &PaneLayout,
    source_pane_id: &str,
    placement: PaneTemplatePlacement,
) -> Option<PredictedLayout> {
    let root = pane_template_target_tree(layout, source_pane_id, placement)?;
    let tree = ocherdr_core::LayoutTree {
        root,
        area: layout.area,
    };
    Some(PredictedLayout {
        panes: tree.pane_rects(),
        splits: tree.splits(),
        tree,
    })
}

pub(crate) fn pane_template_slot_fractions(
    template: PaneLayoutTemplate,
) -> Vec<(f32, f32, f32, f32)> {
    let ids = (0..template.pane_count())
        .map(|index| index.to_string())
        .collect::<Vec<_>>();
    let Some(root) = template_tree(template, &ids) else {
        return Vec::new();
    };
    let tree = ocherdr_core::LayoutTree {
        root,
        area: LayoutRect {
            x: 0,
            y: 0,
            width: 1200,
            height: 1200,
        },
    };
    let mut slots = vec![(0., 0., 0., 0.); ids.len()];
    for pane in tree.pane_rects() {
        let Some(index) = pane.pane_id.parse::<usize>().ok() else {
            continue;
        };
        if let Some(rect) = layout_rect_fractions(tree.area, pane.rect) {
            slots[index] = rect;
        }
    }
    slots
}

fn template_tree(template: PaneLayoutTemplate, ids: &[String]) -> Option<LayoutNode> {
    use PaneLayoutTemplate as T;
    let pane = |index: usize| ids.get(index).cloned().map(LayoutNode::Pane);
    let split = |direction, ratio, first, second| LayoutNode::Split {
        direction,
        ratio,
        first: Box::new(first),
        second: Box::new(second),
    };
    let right = SplitDirection::Right;
    let down = SplitDirection::Down;
    Some(match template {
        T::TwoColumns => split(right, 0.5, pane(0)?, pane(1)?),
        T::TwoRows => split(down, 0.5, pane(0)?, pane(1)?),
        T::ThreeColumns => split(
            right,
            1. / 3.,
            pane(0)?,
            split(right, 0.5, pane(1)?, pane(2)?),
        ),
        T::ThreeRows => split(
            down,
            1. / 3.,
            pane(0)?,
            split(down, 0.5, pane(1)?, pane(2)?),
        ),
        T::ThreeMainLeft => split(right, 0.5, pane(0)?, split(down, 0.5, pane(1)?, pane(2)?)),
        T::ThreeMainRight => split(right, 0.5, split(down, 0.5, pane(0)?, pane(1)?), pane(2)?),
        T::FourGrid => split(
            down,
            0.5,
            split(right, 0.5, pane(0)?, pane(1)?),
            split(right, 0.5, pane(2)?, pane(3)?),
        ),
        T::FourColumns => split(
            right,
            0.25,
            pane(0)?,
            split(
                right,
                1. / 3.,
                pane(1)?,
                split(right, 0.5, pane(2)?, pane(3)?),
            ),
        ),
        T::FourRows => split(
            down,
            0.25,
            pane(0)?,
            split(
                down,
                1. / 3.,
                pane(1)?,
                split(down, 0.5, pane(2)?, pane(3)?),
            ),
        ),
        T::FourMainLeft => split(
            right,
            0.5,
            pane(0)?,
            split(
                down,
                1. / 3.,
                pane(1)?,
                split(down, 0.5, pane(2)?, pane(3)?),
            ),
        ),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneTemplateInsert {
    pub(crate) pane_id: String,
    pub(crate) target_pane_id: String,
    pub(crate) direction: SplitDirection,
    pub(crate) ratio: f32,
}

pub(crate) fn pane_template_construction(
    root: &LayoutNode,
) -> Option<(String, Vec<PaneTemplateInsert>)> {
    let mut reduced = root.clone();
    let mut removals = Vec::new();
    while !matches!(reduced, LayoutNode::Pane(_)) {
        if !prune_second_leaf(&mut reduced, &mut removals) {
            return None;
        }
    }
    let LayoutNode::Pane(anchor) = reduced else {
        return None;
    };
    removals.reverse();
    Some((anchor, removals))
}

fn prune_second_leaf(node: &mut LayoutNode, removals: &mut Vec<PaneTemplateInsert>) -> bool {
    let LayoutNode::Split {
        direction,
        ratio,
        first,
        second,
    } = node
    else {
        return false;
    };
    if let (LayoutNode::Pane(target), LayoutNode::Pane(pane)) = (&**first, &**second) {
        removals.push(PaneTemplateInsert {
            pane_id: pane.clone(),
            target_pane_id: target.clone(),
            direction: *direction,
            ratio: *ratio,
        });
        *node = LayoutNode::Pane(target.clone());
        return true;
    }
    prune_second_leaf(first, removals) || prune_second_leaf(second, removals)
}

pub(crate) fn pane_template_tree_matches(expected: &LayoutNode, actual: &LayoutNode) -> bool {
    match (expected, actual) {
        (LayoutNode::Pane(a), LayoutNode::Pane(b)) => a == b,
        (
            LayoutNode::Split {
                direction: ad,
                ratio: ar,
                first: af,
                second: as_,
            },
            LayoutNode::Split {
                direction: bd,
                ratio: br,
                first: bf,
                second: bs,
            },
        ) => {
            ad == bd
                && (ar - br).abs() < 1e-5
                && pane_template_tree_matches(af, bf)
                && pane_template_tree_matches(as_, bs)
        }
        _ => false,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneTemplateCommitPhase {
    Parking(usize),
    Inserting(usize),
    AwaitingLayout,
}

#[derive(Clone)]
pub(crate) struct PendingPaneTemplateCommit {
    pub(crate) operation_id: u64,
    pub(crate) workspace_id: String,
    pub(crate) tab_id: String,
    pub(crate) source_pane_id: String,
    pub(crate) expected_root: LayoutNode,
    pub(crate) predicted_rects: Vec<PredictedPane>,
    pub(crate) steps: Vec<PaneTemplateInsert>,
    pub(crate) parked: HashMap<String, ParkedPane>,
    pub(crate) known_tab_ids: HashSet<String>,
    pub(crate) phase: PaneTemplateCommitPhase,
}

impl PendingPaneTemplateCommit {
    pub(crate) fn predicted_pane_ids(&self) -> impl Iterator<Item = &str> {
        self.predicted_rects
            .iter()
            .map(|pane| pane.pane_id.as_str())
    }

    pub(crate) fn predicted_fractions(&self, area: LayoutRect) -> Vec<(String, PaneFractions)> {
        self.predicted_rects
            .iter()
            .filter_map(|pane| {
                Some((
                    pane.pane_id.clone(),
                    layout_rect_fractions(area, pane.rect)?,
                ))
            })
            .collect()
    }

    pub(crate) fn hidden_tab_ids(&self, snapshot: &HierarchySnapshot) -> HashSet<String> {
        let pane_ids = self.predicted_pane_ids().collect::<HashSet<_>>();
        snapshot
            .tabs_for(&self.workspace_id)
            .filter(|tab| !self.known_tab_ids.contains(&tab.tab_id))
            .filter(|tab| {
                let panes = snapshot.panes_for(&tab.tab_id).collect::<Vec<_>>();
                panes
                    .iter()
                    .all(|pane| pane_ids.contains(pane.pane_id.as_str()))
            })
            .map(|tab| tab.tab_id.clone())
            .chain(
                self.parked
                    .values()
                    .map(|parked| parked.temp_tab_id.clone()),
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(ids: &[&str]) -> PaneLayout {
        PaneLayout {
            workspace_id: "w".into(),
            tab_id: "t".into(),
            zoomed: false,
            area: LayoutRect {
                x: 0,
                y: 0,
                width: 120,
                height: 80,
            },
            focused_pane_id: ids[0].into(),
            panes: ids
                .iter()
                .map(|id| ocherdr_core::LayoutPane {
                    pane_id: (*id).into(),
                    focused: false,
                    rect: LayoutRect {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                })
                .collect(),
            splits: Vec::new(),
        }
    }

    #[test]
    fn templates_expose_only_the_matching_pane_count() {
        assert_eq!(pane_layout_templates(1), &[]);
        assert_eq!(pane_layout_templates(2).len(), 2);
        assert_eq!(pane_layout_templates(3).len(), 4);
        assert_eq!(pane_layout_templates(4).len(), 4);
        assert_eq!(pane_layout_templates(5), &[]);
    }

    #[test]
    fn the_dragged_pane_occupies_the_selected_template_slot() {
        let layout = layout(&["a", "b", "c", "d"]);
        let predicted = pane_template_predicted_layout(
            &layout,
            "c",
            PaneTemplatePlacement {
                template: PaneLayoutTemplate::FourGrid,
                slot: 0,
            },
        )
        .unwrap();
        assert_eq!(predicted.panes[0].pane_id, "c");
        assert_eq!(predicted.panes[1].pane_id, "a");
        assert_eq!(predicted.panes[2].pane_id, "b");
        assert_eq!(predicted.panes[3].pane_id, "d");
    }

    #[test]
    fn every_template_can_be_rebuilt_by_parking_then_inserting_second_leaves() {
        for count in 2..=4 {
            let ids = (0..count)
                .map(|index| index.to_string())
                .collect::<Vec<_>>();
            for template in pane_layout_templates(count) {
                let root = template_tree(*template, &ids).unwrap();
                let (anchor, steps) = pane_template_construction(&root).unwrap();
                let mut rebuilt = LayoutNode::Pane(anchor);
                for step in steps {
                    rebuilt = ocherdr_core::split_at(
                        rebuilt,
                        &step.target_pane_id,
                        step.direction,
                        &step.pane_id,
                        step.ratio,
                    );
                }
                assert!(pane_template_tree_matches(&root, &rebuilt), "{template:?}");
            }
        }
    }

    #[test]
    fn palette_is_centered_and_each_cell_is_hittable() {
        let surface = (100., 50., 600., 400.);
        let palette = pane_template_palette_geometry(surface, 3).unwrap();
        assert!((palette.rect.0 + palette.rect.2 / 2. - 400.).abs() < 1e-6);
        for card in palette.cards {
            assert_eq!(card.slots.len(), 3);
            for (slot, rect) in card.slots.into_iter().enumerate() {
                let hover =
                    pane_template_hover(surface, 3, (rect.0 + rect.2 / 2., rect.1 + rect.3 / 2.))
                        .unwrap();
                assert_eq!(hover.placement.template, card.template);
                assert_eq!(hover.placement.slot, slot);
            }
        }
    }
}
