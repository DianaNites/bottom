use std::{borrow::Cow, collections::hash_map::Entry};

use crate::{
    app::{
        data_farmer::{DataCollection, ProcessData},
        data_harvester::processes::ProcessHarvest,
        query::*,
        AppConfigFields, AppSearchState,
    },
    canvas::canvas_colours::CanvasColours,
    components::data_table::{
        Column, ColumnHeader, ColumnWidthBounds, DataTable, DataTableColumn, DataTableProps,
        DataTableStyling, SortColumn, SortDataTable, SortDataTableProps, SortOrder,
    },
    Pid,
};

use fxhash::{FxHashMap, FxHashSet};
use itertools::Itertools;

pub mod proc_widget_column;
pub use proc_widget_column::*;

pub mod proc_widget_data;
pub use proc_widget_data::*;

mod sort_table;
use sort_table::SortTableColumn;

/// ProcessSearchState only deals with process' search's current settings and state.
pub struct ProcessSearchState {
    pub search_state: AppSearchState,
    pub is_ignoring_case: bool,
    pub is_searching_whole_word: bool,
    pub is_searching_with_regex: bool,
}

impl Default for ProcessSearchState {
    fn default() -> Self {
        ProcessSearchState {
            search_state: AppSearchState::default(),
            is_ignoring_case: true,
            is_searching_whole_word: false,
            is_searching_with_regex: false,
        }
    }
}

impl ProcessSearchState {
    pub fn search_toggle_ignore_case(&mut self) {
        self.is_ignoring_case = !self.is_ignoring_case;
    }

    pub fn search_toggle_whole_word(&mut self) {
        self.is_searching_whole_word = !self.is_searching_whole_word;
    }

    pub fn search_toggle_regex(&mut self) {
        self.is_searching_with_regex = !self.is_searching_with_regex;
    }
}

#[derive(Clone, Debug)]
pub enum ProcWidgetMode {
    Tree { collapsed_pids: FxHashSet<Pid> },
    Grouped,
    Normal,
}

type ProcessTable = SortDataTable<ProcWidgetData, ProcColumn>;
type SortTable = DataTable<Cow<'static, str>, SortTableColumn>;
type StringPidMap = FxHashMap<String, Vec<Pid>>;

pub struct ProcWidget {
    pub mode: ProcWidgetMode,

    /// The state of the search box.
    pub proc_search: ProcessSearchState,

    /// The state of the main table.
    pub table: ProcessTable,

    /// The stored process data for this specific table.
    pub table_data: Vec<ProcWidgetData>,

    /// The state of the togglable table that controls sorting.
    pub sort_table: SortTable,

    /// A name-to-pid mapping.
    pub id_pid_map: StringPidMap,

    pub is_sort_open: bool,
    pub force_rerender: bool,
    pub force_update_data: bool,
}

impl ProcWidget {
    pub const PID_OR_COUNT: usize = 0;
    pub const PROC_NAME_OR_CMD: usize = 1;
    pub const CPU: usize = 2;
    pub const MEM: usize = 3;
    pub const RPS: usize = 4;
    pub const WPS: usize = 5;
    pub const T_READ: usize = 6;
    pub const T_WRITE: usize = 7;
    #[cfg(target_family = "unix")]
    pub const USER: usize = 8;
    #[cfg(target_family = "unix")]
    pub const STATE: usize = 9;
    #[cfg(not(target_family = "unix"))]
    pub const STATE: usize = 8;

    fn new_sort_table(config: &AppConfigFields, colours: &CanvasColours) -> SortTable {
        const COLUMNS: [Column<SortTableColumn>; 1] = [Column::hard(SortTableColumn, 7)];

        let props = DataTableProps {
            title: None,
            table_gap: config.table_gap,
            left_to_right: true,
            is_basic: false,
            show_table_scroll_position: false,
            show_current_entry_when_unfocused: false,
        };

        let styling = DataTableStyling::from_colours(colours);

        DataTable::new(COLUMNS, props, styling)
    }

    fn new_process_table(
        config: &AppConfigFields, colours: &CanvasColours, mode: &ProcWidgetMode, is_count: bool,
        is_command: bool, show_memory_as_values: bool,
    ) -> ProcessTable {
        let (default_index, default_order) = if matches!(mode, ProcWidgetMode::Tree { .. }) {
            (Self::PID_OR_COUNT, SortOrder::Ascending)
        } else {
            (Self::CPU, SortOrder::Descending)
        };

        let columns = {
            use ProcColumn::*;

            let pid_or_count = SortColumn::new(if is_count { Count } else { Pid });
            let name_or_cmd = SortColumn::soft(if is_command { Command } else { Name }, Some(0.3));
            let cpu = SortColumn::new(CpuPercent).default_descending();
            let mem = SortColumn::new(if show_memory_as_values {
                MemoryVal
            } else {
                MemoryPercent
            })
            .default_descending();
            let rps = SortColumn::hard(ReadPerSecond, 8).default_descending();
            let wps = SortColumn::hard(WritePerSecond, 8).default_descending();
            let tr = SortColumn::hard(TotalRead, 8).default_descending();
            let tw = SortColumn::hard(TotalWrite, 8).default_descending();
            let state = SortColumn::hard(State, 7);

            vec![
                pid_or_count,
                name_or_cmd,
                cpu,
                mem,
                rps,
                wps,
                tr,
                tw,
                #[cfg(target_family = "unix")]
                SortColumn::soft(User, Some(0.05)),
                state,
            ]
        };

        let inner_props = DataTableProps {
            title: Some(" Processes ".into()),
            table_gap: config.table_gap,
            left_to_right: true,
            is_basic: config.use_basic_mode,
            show_table_scroll_position: config.show_table_scroll_position,
            show_current_entry_when_unfocused: false,
        };
        let props = SortDataTableProps {
            inner: inner_props,
            sort_index: default_index,
            order: default_order,
        };

        let styling = DataTableStyling::from_colours(colours);

        DataTable::new_sortable(columns, props, styling)
    }

    pub fn new(
        config: &AppConfigFields, mode: ProcWidgetMode, is_case_sensitive: bool,
        is_match_whole_word: bool, is_use_regex: bool, show_memory_as_values: bool,
        is_command: bool, colours: &CanvasColours,
    ) -> Self {
        let process_search_state = {
            let mut pss = ProcessSearchState::default();

            if is_case_sensitive {
                // By default it's off
                pss.search_toggle_ignore_case();
            }
            if is_match_whole_word {
                pss.search_toggle_whole_word();
            }
            if is_use_regex {
                pss.search_toggle_regex();
            }

            pss
        };

        let is_count = matches!(mode, ProcWidgetMode::Grouped);
        let sort_table = Self::new_sort_table(config, colours);
        let table = Self::new_process_table(
            config,
            colours,
            &mode,
            is_count,
            is_command,
            show_memory_as_values,
        );

        let id_pid_map = FxHashMap::default();

        ProcWidget {
            proc_search: process_search_state,
            table,
            table_data: vec![],
            sort_table,
            id_pid_map,
            is_sort_open: false,
            mode,
            force_rerender: true,
            force_update_data: false,
        }
    }

    pub fn is_using_command(&self) -> bool {
        self.table
            .columns
            .get(ProcWidget::PROC_NAME_OR_CMD)
            .map(|col| matches!(col.inner(), ProcColumn::Command))
            .unwrap_or(false)
    }

    pub fn is_mem_percent(&self) -> bool {
        self.table
            .columns
            .get(ProcWidget::MEM)
            .map(|col| matches!(col.inner(), ProcColumn::MemoryPercent))
            .unwrap_or(false)
    }

    fn get_query(&self) -> &Option<Query> {
        if self.proc_search.search_state.is_invalid_or_blank_search() {
            &None
        } else {
            &self.proc_search.search_state.query
        }
    }

    /// This function *only* updates the displayed process data. If there is a need to update the actual *stored* data,
    /// call it before this function.
    pub fn update_displayed_process_data(&mut self, data_collection: &DataCollection) {
        self.table_data = match &self.mode {
            ProcWidgetMode::Grouped | ProcWidgetMode::Normal => {
                self.get_normal_data(&data_collection.process_data.process_harvest)
            }
            ProcWidgetMode::Tree { collapsed_pids } => {
                self.get_tree_data(collapsed_pids, data_collection)
            }
        };
    }

    fn get_tree_data(
        &self, collapsed_pids: &FxHashSet<Pid>, data_collection: &DataCollection,
    ) -> Vec<ProcWidgetData> {
        const BRANCH_END: char = '└';
        const BRANCH_VERTICAL: char = '│';
        const BRANCH_SPLIT: char = '├';
        const BRANCH_HORIZONTAL: char = '─';

        let search_query = self.get_query();
        let is_using_command = self.is_using_command();
        let is_mem_percent = self.is_mem_percent();

        let ProcessData {
            process_harvest,
            process_parent_mapping,
            orphan_pids,
            ..
        } = &data_collection.process_data;

        let kept_pids = data_collection
            .process_data
            .process_harvest
            .iter()
            .map(|(pid, process)| {
                (
                    *pid,
                    search_query
                        .as_ref()
                        .map(|q| q.check(process, is_using_command))
                        .unwrap_or(true),
                )
            })
            .collect::<FxHashMap<_, _>>();

        let filtered_tree = {
            let mut filtered_tree = FxHashMap::default();

            // We do a simple BFS traversal to build our filtered parent-to-tree mappings.
            let mut visited_pids = FxHashMap::default();
            let mut stack = orphan_pids
                .iter()
                .filter_map(|process| process_harvest.get(process))
                .collect_vec();

            while let Some(process) = stack.last() {
                let is_process_matching = *kept_pids.get(&process.pid).unwrap_or(&false);

                if let Some(children_pids) = process_parent_mapping.get(&process.pid) {
                    if children_pids
                        .iter()
                        .all(|pid| visited_pids.contains_key(pid))
                    {
                        let shown_children = children_pids
                            .iter()
                            .filter(|pid| visited_pids.get(*pid).copied().unwrap_or(false))
                            .collect_vec();
                        let is_shown = is_process_matching || !shown_children.is_empty();
                        visited_pids.insert(process.pid, is_shown);

                        if is_shown {
                            filtered_tree.insert(
                                process.pid,
                                shown_children
                                    .into_iter()
                                    .filter_map(|pid| {
                                        process_harvest.get(pid).map(|process| process.pid)
                                    })
                                    .collect_vec(),
                            );
                        }

                        stack.pop();
                    } else {
                        children_pids
                            .iter()
                            .filter_map(|process| process_harvest.get(process))
                            .rev()
                            .for_each(|process| {
                                stack.push(process);
                            });
                    }
                } else {
                    if is_process_matching {
                        filtered_tree.insert(process.pid, vec![]);
                    }

                    visited_pids.insert(process.pid, is_process_matching);
                    stack.pop();
                }
            }

            filtered_tree
        };

        let mut data = vec![];
        let mut prefixes = vec![];
        let mut stack = orphan_pids
            .iter()
            .filter_map(|pid| {
                if filtered_tree.contains_key(pid) {
                    process_harvest.get(pid).map(|process| {
                        ProcWidgetData::from_data(process, is_using_command, is_mem_percent)
                    })
                } else {
                    None
                }
            })
            .collect_vec();

        self.try_sort(&mut stack);

        let mut length_stack = vec![stack.len()];

        while let (Some(process), Some(siblings_left)) = (stack.pop(), length_stack.last_mut()) {
            *siblings_left -= 1;

            let disabled = !*kept_pids.get(&process.pid).unwrap_or(&false);
            let is_last = *siblings_left == 0;

            if collapsed_pids.contains(&process.pid) {
                let mut summed_process = process.clone();

                if let Some(children_pids) = filtered_tree.get(&process.pid) {
                    let mut sum_queue = children_pids
                        .iter()
                        .filter_map(|child| {
                            process_harvest.get(child).map(|p| {
                                ProcWidgetData::from_data(p, is_using_command, is_mem_percent)
                            })
                        })
                        .collect_vec();

                    while let Some(process) = sum_queue.pop() {
                        summed_process.add(&process);

                        if let Some(pids) = filtered_tree.get(&process.pid) {
                            sum_queue.extend(pids.iter().filter_map(|child| {
                                process_harvest.get(child).map(|p| {
                                    ProcWidgetData::from_data(p, is_using_command, is_mem_percent)
                                })
                            }));
                        }
                    }
                }

                let prefix = if prefixes.is_empty() {
                    "+ ".to_string()
                } else {
                    format!(
                        "{}{}{} + ",
                        prefixes.join(""),
                        if is_last { BRANCH_END } else { BRANCH_SPLIT },
                        BRANCH_HORIZONTAL
                    )
                };

                data.push(summed_process.prefix(Some(prefix)).disabled(disabled));
            } else {
                let prefix = if prefixes.is_empty() {
                    String::default()
                } else {
                    format!(
                        "{}{}{} ",
                        prefixes.join(""),
                        if is_last { BRANCH_END } else { BRANCH_SPLIT },
                        BRANCH_HORIZONTAL
                    )
                };
                let pid = process.pid;
                data.push(process.prefix(Some(prefix)).disabled(disabled));

                if let Some(children_pids) = filtered_tree.get(&pid) {
                    if prefixes.is_empty() {
                        prefixes.push(String::default());
                    } else {
                        prefixes.push(if is_last {
                            "   ".to_string()
                        } else {
                            format!("{}  ", BRANCH_VERTICAL)
                        });
                    }

                    let mut children = children_pids
                        .iter()
                        .filter_map(|child_pid| {
                            process_harvest.get(child_pid).map(|p| {
                                ProcWidgetData::from_data(p, is_using_command, is_mem_percent)
                            })
                        })
                        .collect_vec();
                    self.try_rev_sort(&mut children);
                    length_stack.push(children.len());
                    stack.extend(children);
                }
            }

            while let Some(children_left) = length_stack.last() {
                if *children_left == 0 {
                    length_stack.pop();
                    prefixes.pop();
                } else {
                    break;
                }
            }
        }

        data
    }

    fn get_normal_data(
        &mut self, process_harvest: &FxHashMap<Pid, ProcessHarvest>,
    ) -> Vec<ProcWidgetData> {
        let search_query = self.get_query();
        let is_using_command = self.is_using_command();
        let is_mem_percent = self.is_mem_percent();

        let filtered_iter = process_harvest.values().filter(|process| {
            search_query
                .as_ref()
                .map(|query| query.check(process, is_using_command))
                .unwrap_or(true)
        });

        let mut id_pid_map: FxHashMap<String, Vec<Pid>> = FxHashMap::default();
        let mut filtered_data: Vec<ProcWidgetData> = if let ProcWidgetMode::Grouped = self.mode {
            let mut id_process_mapping: FxHashMap<String, ProcessHarvest> = FxHashMap::default();
            for process in filtered_iter {
                let id = if is_using_command {
                    &process.command
                } else {
                    &process.name
                };
                let pid = process.pid;

                match id_pid_map.entry(id.clone()) {
                    Entry::Occupied(mut occupied) => {
                        occupied.get_mut().push(pid);
                    }
                    Entry::Vacant(vacant) => {
                        vacant.insert(vec![pid]);
                    }
                }

                if let Some(grouped_process_harvest) = id_process_mapping.get_mut(id) {
                    grouped_process_harvest.add(process);
                } else {
                    id_process_mapping.insert(id.clone(), process.clone());
                }
            }

            id_process_mapping
                .values()
                .map(|process| {
                    let id = if is_using_command {
                        &process.command
                    } else {
                        &process.name
                    };

                    let num_similar = id_pid_map.get(id).map(|val| val.len()).unwrap_or(1) as u64;

                    ProcWidgetData::from_data(process, is_using_command, is_mem_percent)
                        .num_similar(num_similar)
                })
                .collect()
        } else {
            filtered_iter
                .map(|process| ProcWidgetData::from_data(process, is_using_command, is_mem_percent))
                .collect()
        };

        self.id_pid_map = id_pid_map;
        self.try_sort(&mut filtered_data);
        filtered_data
    }

    #[inline(always)]
    fn try_sort(&self, filtered_data: &mut [ProcWidgetData]) {
        if let Some(column) = self.table.columns.get(self.table.sort_index()) {
            column.sort_by(filtered_data, self.table.order());
        }
    }

    #[inline(always)]
    fn try_rev_sort(&self, filtered_data: &mut [ProcWidgetData]) {
        if let Some(column) = self.table.columns.get(self.table.sort_index()) {
            column.sort_by(
                filtered_data,
                match self.table.order() {
                    SortOrder::Ascending => SortOrder::Descending,
                    SortOrder::Descending => SortOrder::Ascending,
                },
            );
        }
    }

    #[inline(always)]
    fn get_mut_proc_col(&mut self, index: usize) -> Option<&mut ProcColumn> {
        self.table.columns.get_mut(index).map(|col| col.inner_mut())
    }

    pub fn toggle_mem_percentage(&mut self) {
        if let Some(mem) = self.get_mut_proc_col(Self::MEM) {
            match mem {
                ProcColumn::MemoryVal => {
                    *mem = ProcColumn::MemoryPercent;
                }
                ProcColumn::MemoryPercent => {
                    *mem = ProcColumn::MemoryVal;
                }
                _ => unreachable!(),
            }

            self.force_data_update();
        }
    }

    /// Forces an update of the data stored.
    #[inline]
    pub fn force_data_update(&mut self) {
        self.force_update_data = true;
    }

    /// Forces an entire rerender and update of the data stored.
    #[inline]
    pub fn force_rerender_and_update(&mut self) {
        self.force_rerender = true;
        self.force_update_data = true;
    }

    /// Marks the selected column as hidden, and automatically resets the selected column to CPU
    /// and descending if that column was selected.
    fn hide_column(&mut self, index: usize) {
        if let Some(col) = self.table.columns.get_mut(index) {
            col.is_hidden = true;

            if self.table.sort_index() == index {
                self.table.set_sort_index(Self::CPU);
                self.table.set_order(SortOrder::Descending);
            }
        }
    }

    /// Marks the selected column as shown.
    fn show_column(&mut self, index: usize) {
        if let Some(col) = self.table.columns.get_mut(index) {
            col.is_hidden = false;
        }
    }

    /// Select a column. If the column is already selected, then just toggle the sort order.
    pub fn select_column(&mut self, new_sort_index: usize) {
        self.table.set_sort_index(new_sort_index);
        self.force_data_update();
    }

    pub fn toggle_current_tree_branch_entry(&mut self) {
        if let ProcWidgetMode::Tree { collapsed_pids } = &mut self.mode {
            if let Some(process) = self.table.current_item() {
                let pid = process.pid;

                if !collapsed_pids.remove(&pid) {
                    collapsed_pids.insert(pid);
                }
                self.force_data_update();
            }
        }
    }

    pub fn toggle_command(&mut self) {
        if let Some(col) = self.table.columns.get_mut(Self::PROC_NAME_OR_CMD) {
            let inner = col.inner_mut();
            match inner {
                ProcColumn::Name => {
                    *inner = ProcColumn::Command;
                    if let ColumnWidthBounds::Soft { max_percentage, .. } = col.bounds_mut() {
                        *max_percentage = Some(0.5);
                    }
                }
                ProcColumn::Command => {
                    *inner = ProcColumn::Name;
                    if let ColumnWidthBounds::Soft { max_percentage, .. } = col.bounds_mut() {
                        *max_percentage = match self.mode {
                            ProcWidgetMode::Tree { .. } => Some(0.5),
                            ProcWidgetMode::Grouped | ProcWidgetMode::Normal => Some(0.3),
                        };
                    }
                }
                _ => unreachable!(),
            }
            self.force_rerender_and_update();
        }
    }

    /// Toggles the appropriate columns/settings when tab is pressed.
    ///
    /// If count is enabled, we should set the mode to [`ProcWidgetMode::Grouped`], and switch off the User and State
    /// columns. We should also move the user off of the columns if they were selected, as those columns are now hidden
    /// (handled by internal method calls), and go back to the "defaults".
    ///
    /// Otherwise, if count is disabled, then the User and State columns should be re-enabled, and the mode switched
    /// to [`ProcWidgetMode::Normal`].
    pub fn on_tab(&mut self) {
        if !matches!(self.mode, ProcWidgetMode::Tree { .. }) {
            if let Some(sort_col) = self.table.columns.get_mut(Self::PID_OR_COUNT) {
                let col = sort_col.inner_mut();
                match col {
                    ProcColumn::Pid => {
                        *col = ProcColumn::Count;
                        sort_col.default_order = SortOrder::Descending;

                        #[cfg(target_family = "unix")]
                        self.hide_column(Self::USER);
                        self.hide_column(Self::STATE);
                        self.mode = ProcWidgetMode::Grouped;
                    }
                    ProcColumn::Count => {
                        *col = ProcColumn::Pid;
                        sort_col.default_order = SortOrder::Ascending;

                        #[cfg(target_family = "unix")]
                        self.show_column(Self::USER);
                        self.show_column(Self::STATE);
                        self.mode = ProcWidgetMode::Normal;
                    }
                    _ => unreachable!(),
                }

                self.force_rerender_and_update();
            }
        }
    }

    pub fn column_text(&self) -> Vec<Cow<'static, str>> {
        self.table
            .columns
            .iter()
            .filter(|c| !c.is_hidden)
            .map(|c| c.inner().text())
            .collect::<Vec<_>>()
    }

    pub fn get_search_cursor_position(&self) -> usize {
        self.proc_search.search_state.grapheme_cursor.cur_cursor()
    }

    pub fn get_char_cursor_position(&self) -> usize {
        self.proc_search.search_state.char_cursor_position
    }

    pub fn is_search_enabled(&self) -> bool {
        self.proc_search.search_state.is_enabled
    }

    pub fn get_current_search_query(&self) -> &String {
        &self.proc_search.search_state.current_search_query
    }

    pub fn update_query(&mut self) {
        if self
            .proc_search
            .search_state
            .current_search_query
            .is_empty()
        {
            self.proc_search.search_state.is_blank_search = true;
            self.proc_search.search_state.is_invalid_search = false;
            self.proc_search.search_state.error_message = None;
        } else {
            match parse_query(
                &self.proc_search.search_state.current_search_query,
                self.proc_search.is_searching_whole_word,
                self.proc_search.is_ignoring_case,
                self.proc_search.is_searching_with_regex,
            ) {
                Ok(parsed_query) => {
                    self.proc_search.search_state.query = Some(parsed_query);
                    self.proc_search.search_state.is_blank_search = false;
                    self.proc_search.search_state.is_invalid_search = false;
                    self.proc_search.search_state.error_message = None;
                }
                Err(err) => {
                    self.proc_search.search_state.is_blank_search = false;
                    self.proc_search.search_state.is_invalid_search = true;
                    self.proc_search.search_state.error_message = Some(err.to_string());
                }
            }
        }
        self.table.state.display_start_index = 0;
        self.table.state.current_index = 0;

        self.force_data_update();
    }

    pub fn clear_search(&mut self) {
        self.proc_search.search_state.reset();
        self.force_data_update();
    }

    pub fn search_walk_forward(&mut self, start_position: usize) {
        self.proc_search
            .search_state
            .grapheme_cursor
            .next_boundary(
                &self.proc_search.search_state.current_search_query[start_position..],
                start_position,
            )
            .unwrap();
    }

    pub fn search_walk_back(&mut self, start_position: usize) {
        self.proc_search
            .search_state
            .grapheme_cursor
            .prev_boundary(
                &self.proc_search.search_state.current_search_query[..start_position],
                0,
            )
            .unwrap();
    }

    /// Returns the number of columns *enabled*. Note this differs from *visible* - a column may be enabled but not
    /// visible (e.g. off screen).
    pub fn num_enabled_columns(&self) -> usize {
        self.table.columns.iter().filter(|c| !c.is_hidden).count()
    }

    /// Sets the [`ProcWidget`]'s current sort index to whatever was in the sort table if possible, then closes the
    /// sort table.
    pub(crate) fn use_sort_table_value(&mut self) {
        self.table.set_sort_index(self.sort_table.current_index());

        self.is_sort_open = false;
        self.force_rerender_and_update();
    }
}
