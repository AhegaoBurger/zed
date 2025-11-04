use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use gpui::{
    App, Entity, EventEmitter, FocusHandle, Focusable,
    SharedString, Task, Window, prelude::*,
};
use language::LanguageRegistry;
use project::{Project, ProjectEntryId, ProjectPath};
use ui::prelude::*;
use workspace::{Item, ProjectItem};

use crate::outputs::Output;

/// Represents a single cell in a marimo notebook
#[derive(Debug, Clone)]
pub struct MarimoCell {
    /// The function name (cell name)
    pub name: Option<String>,
    /// The cell's Python code content
    pub code: String,
    /// Dependencies (function parameters)
    pub dependencies: Vec<String>,
    /// Outputs from execution (if any)
    pub outputs: Vec<Output>,
}

/// Represents a marimo notebook file (.py with marimo structure)
#[derive(Debug, Clone)]
pub struct MarimoNotebook {
    /// The marimo version that generated this file
    pub version: Option<String>,
    /// All cells in the notebook
    pub cells: Vec<MarimoCell>,
    /// Raw file content
    pub content: String,
}

impl MarimoNotebook {
    /// Parse a marimo notebook from Python source code
    pub fn parse(content: &str) -> Result<Self> {
        // Check if this is a marimo notebook
        if !Self::is_marimo_notebook(content) {
            bail!("Not a valid marimo notebook");
        }

        let mut cells = Vec::new();
        let mut version = None;

        // Extract version if present
        if let Some(caps) = regex::Regex::new(r#"__generated_with\s*=\s*["']([^"']+)["']"#)
            .ok()
            .and_then(|re| re.captures(content))
        {
            version = Some(caps[1].to_string());
        }

        // Parse cells using a simple regex approach
        // Look for @app.cell decorated functions
        let cell_regex = regex::Regex::new(
            r"(?m)@app\.cell(?:\([^)]*\))?\s*\ndef\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*\(([^)]*)\)\s*:\s*\n((?:    .*\n)*)"
        ).context("Failed to compile cell regex")?;

        for caps in cell_regex.captures_iter(content) {
            let name = caps.get(1).map(|m| m.as_str().to_string());
            let params = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let body = caps.get(3).map(|m| m.as_str()).unwrap_or("");

            // Parse dependencies from function parameters
            let dependencies: Vec<String> = params
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();

            // Remove leading indentation from body
            let code = body
                .lines()
                .map(|line| line.strip_prefix("    ").unwrap_or(line))
                .collect::<Vec<_>>()
                .join("\n");

            cells.push(MarimoCell {
                name,
                code,
                dependencies,
                outputs: Vec::new(),
            });
        }

        if cells.is_empty() {
            bail!("No cells found in marimo notebook");
        }

        Ok(Self {
            version,
            cells,
            content: content.to_string(),
        })
    }

    /// Check if a Python file is a marimo notebook
    pub fn is_marimo_notebook(content: &str) -> bool {
        // A marimo notebook should have:
        // 1. import marimo
        // 2. app = marimo.App() or similar
        // 3. @app.cell decorators

        let has_marimo_import = content.contains("import marimo");
        let has_app_creation = content.contains("marimo.App(") || content.contains("= marimo.App");
        let has_app_cell = content.contains("@app.cell");

        has_marimo_import && (has_app_creation || has_app_cell)
    }
}

/// ProjectItem implementation for marimo notebooks
pub struct MarimoNotebookItem {
    path: PathBuf,
    project_path: ProjectPath,
    languages: Arc<LanguageRegistry>,
    notebook: MarimoNotebook,
    id: ProjectEntryId,
}

impl MarimoNotebookItem {
    pub fn notebook(&self) -> &MarimoNotebook {
        &self.notebook
    }

    pub fn language_name(&self) -> Option<String> {
        // Marimo notebooks are always Python
        Some("Python".to_string())
    }
}

impl project::ProjectItem for MarimoNotebookItem {
    fn try_open(
        project: &Entity<Project>,
        path: &ProjectPath,
        cx: &mut App,
    ) -> Option<Task<Result<Entity<Self>>>> {
        let path = path.clone();
        let project = project.clone();
        let fs = project.read(cx).fs().clone();
        let languages = project.read(cx).languages().clone();

        // Only handle .py files
        if path.path.extension().unwrap_or_default() != "py" {
            return None;
        }

        Some(cx.spawn(async move |cx| {
            let abs_path = project
                .read_with(cx, |project, cx| project.absolute_path(&path, cx))?
                .with_context(|| format!("finding the absolute path of {path:?}"))?;

            // Load file content
            let file_content = fs.load(abs_path.as_path()).await?;

            // Check if this is a marimo notebook before trying to parse
            if !MarimoNotebook::is_marimo_notebook(&file_content) {
                // Not a marimo notebook, return None-equivalent error
                bail!("Not a marimo notebook");
            }

            // Try to parse as marimo notebook
            let notebook = MarimoNotebook::parse(&file_content)
                .context("Failed to parse as marimo notebook")?;

            let id = project
                .update(cx, |project, cx| {
                    project.entry_for_path(&path, cx).map(|entry| entry.id)
                })?
                .context("Entry not found")?;

            cx.new(|_| MarimoNotebookItem {
                path: abs_path,
                project_path: path,
                languages,
                notebook,
                id,
            })
        }))
    }

    fn entry_id(&self, _: &App) -> Option<ProjectEntryId> {
        Some(self.id)
    }

    fn project_path(&self, _: &App) -> Option<ProjectPath> {
        Some(self.project_path.clone())
    }
}

/// UI Editor for marimo notebooks
pub struct MarimoNotebookEditor {
    languages: Arc<LanguageRegistry>,
    project: Entity<Project>,
    focus_handle: FocusHandle,
    notebook_item: Entity<MarimoNotebookItem>,
}

impl MarimoNotebookEditor {
    pub fn new(
        notebook_item: Entity<MarimoNotebookItem>,
        project: Entity<Project>,
        languages: Arc<LanguageRegistry>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        Self {
            languages,
            project,
            focus_handle,
            notebook_item,
        }
    }

    pub fn for_project_item(
        project: Entity<Project>,
        item: Entity<MarimoNotebookItem>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let languages = item.read(cx).languages.clone();
        cx.new(|cx| Self::new(item, project, languages, window, cx))
    }
}

impl Focusable for MarimoNotebookEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl gpui::EventEmitter<()> for MarimoNotebookEditor {}

impl Render for MarimoNotebookEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let notebook = self.notebook_item.read(cx).notebook();

        div()
            .id("marimo-notebook-editor")
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().colors().border)
                    .child(
                        div()
                            .text_ui()
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("Marimo Notebook (Read-Only)")
                    )
                    .when_some(notebook.version.as_ref(), |this, version| {
                        this.child(
                            div()
                                .text_ui_sm()
                                .text_color(cx.theme().colors().text_muted)
                                .child(format!("marimo v{}", version))
                        )
                    })
            )
            .child(
                div()
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .children(notebook.cells.iter().enumerate().map(|(ix, cell)| {
                        let cell_name = cell.name.as_ref()
                            .map(|n| {
                                if cell.dependencies.is_empty() {
                                    format!("def {}()", n)
                                } else {
                                    format!("def {}({})", n, cell.dependencies.join(", "))
                                }
                            })
                            .unwrap_or_else(|| "Cell".to_string());

                        div()
                            .id(("marimo-cell", ix))
                            .p_4()
                            .mb_2()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_md()
                            .child(
                                div()
                                    .text_ui_sm()
                                    .text_color(cx.theme().colors().text_muted)
                                    .mb_2()
                                    .child(cell_name)
                            )
                            .child(
                                div()
                                    .font_family("monospace")
                                    .text_ui_sm()
                                    .p_2()
                                    .bg(cx.theme().colors().editor_background)
                                    .rounded_sm()
                                    .child(cell.code.clone())
                            )
                    }))
            )
    }
}

impl workspace::Item for MarimoNotebookEditor {
    type Event = ();

    fn tab_content_text(&self, _detail: Option<usize>, cx: &App) -> Option<SharedString> {
        let path = &self.notebook_item.read(cx).path;
        path.file_name()
            .and_then(|name| name.to_str())
            .map(|s| s.to_string().into())
    }

    fn telemetry_event_text(&self) -> Option<&'static str> {
        Some("marimo notebook")
    }

    fn as_searchable(&self, _handle: &Entity<Self>) -> Option<Box<dyn workspace::SearchableItemHandle>> {
        None
    }

    fn set_nav_history(&mut self, _: workspace::ItemNavHistory, _: &mut App) {}

    fn clone_on_split(
        &self,
        _workspace_id: Option<workspace::WorkspaceId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<Self>>
    where
        Self: Sized,
    {
        Some(Self::for_project_item(
            self.project.clone(),
            self.notebook_item.clone(),
            window,
            cx,
        ))
    }

    fn is_dirty(&self, _cx: &App) -> bool {
        false
    }

    fn has_conflict(&self, _cx: &App) -> bool {
        false
    }

    fn can_save(&self, _cx: &App) -> bool {
        false
    }

    fn save(
        &mut self,
        _format: bool,
        _project: Entity<Project>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn save_as(
        &mut self,
        _project: Entity<Project>,
        _path: ProjectPath,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    fn reload(
        &mut self,
        _project: Entity<Project>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }
}

impl workspace::ProjectItem for MarimoNotebookEditor {
    type Item = MarimoNotebookItem;

    fn for_project_item(
        project: Entity<Project>,
        item: Entity<Self::Item>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self>
    where
        Self: Sized,
    {
        Self::for_project_item(project, item, window, cx)
    }
}

/// Initialize marimo notebook support
pub fn marimo_init(cx: &mut App) {
    workspace::register_project_item::<MarimoNotebookEditor>(cx);
}
