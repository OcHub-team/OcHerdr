use std::collections::HashSet;

use super::*;

/// Exact predicted geometry of one tab in a one-shot tab-bar transfer.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneTransferTabState {
    pub(crate) tab_id: String,
    pub(crate) fingerprint: u64,
    pub(crate) topology: SplitLayoutFingerprint,
    pub(crate) area: LayoutRect,
    pub(crate) predicted_rects: Vec<PredictedPane>,
    pub(crate) predicted_topology: SplitLayoutFingerprint,
}

impl PaneTransferTabState {
    pub(crate) fn predicted_pane_ids(&self) -> impl Iterator<Item = &str> {
        self.predicted_rects
            .iter()
            .map(|pane| pane.pane_id.as_str())
    }

    pub(crate) fn predicted_fractions(&self) -> Vec<(String, PaneFractions)> {
        predicted_pane_fractions(self.area, self.predicted_rects.clone())
    }
}

/// One-shot `pane.move` to a tab-bar target. Independent of the three-step
/// insert orchestration: the created or existing destination tab is real
/// and stays visible.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PendingPaneDetach {
    pub(crate) operation_id: u64,
    pub(crate) workspace_id: String,
    pub(crate) source_pane_id: String,
    pub(crate) destination: PaneTabDropTarget,
    pub(crate) source: PaneTransferTabState,
    pub(crate) target: Option<PaneTransferTabState>,
    pub(crate) known_tab_ids: HashSet<String>,
    pub(crate) responded: bool,
    pub(crate) accepted: bool,
    pub(crate) created_tab_id: Option<String>,
}

impl PendingPaneDetach {
    pub(crate) fn locks_tab(&self, tab_id: &str) -> bool {
        self.source.tab_id == tab_id || self.target.as_ref().is_some_and(|tab| tab.tab_id == tab_id)
    }

    pub(crate) fn tab_state(&self, tab_id: &str) -> Option<&PaneTransferTabState> {
        if self.source.tab_id == tab_id {
            Some(&self.source)
        } else {
            self.target.as_ref().filter(|tab| tab.tab_id == tab_id)
        }
    }

    pub(crate) fn predicted_pane_ids(&self) -> impl Iterator<Item = &str> {
        self.source
            .predicted_pane_ids()
            .chain(self.target.iter().flat_map(|tab| tab.predicted_pane_ids()))
    }

    pub(crate) fn created_tab_from(&self, snapshot: &HierarchySnapshot) -> Option<String> {
        if let Some(tab_id) = self.created_tab_id.as_ref() {
            return Some(tab_id.clone());
        }
        snapshot.pane(&self.source_pane_id).and_then(|pane| {
            (!self.known_tab_ids.contains(&pane.tab_id)).then(|| pane.tab_id.clone())
        })
    }
}
