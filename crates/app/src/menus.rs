//! The application menu bar (macOS) — File, Edit, View, Navigate, Code, Laravel, Run, Git,
//! Window, Help — built from action names so shortcuts show from the keymap.

use gpui::{App, Menu, MenuItem, SystemMenuType};

fn item(cx: &App, name: &str, action: &str) -> Option<MenuItem> {
    let action = cx.build_action(action, None).ok()?;
    Some(MenuItem::Action { name: name.to_string().into(), action, os_action: None })
}

fn menu(cx: &App, name: &str, entries: &[Option<(&str, &str)>]) -> Menu {
    Menu {
        name: name.to_string().into(),
        items: entries
            .iter()
            .filter_map(|entry| match entry {
                None => Some(MenuItem::separator()),
                Some((label, action)) => item(cx, label, action),
            })
            .collect(),
    }
}

pub fn set(cx: &mut App) {
    let menus = vec![
        Menu {
            name: "PhpCraft".into(),
            items: [
                item(cx, "About PhpCraft", "workspace::About"),
                Some(MenuItem::separator()),
                item(cx, "Settings…", "workspace::OpenSettings"),
                item(cx, "Keymap…", "workspace::OpenKeymap"),
                item(cx, "Themes…", "workspace::OpenThemes"),
                Some(MenuItem::separator()),
                Some(MenuItem::os_submenu("Services", SystemMenuType::Services)),
                Some(MenuItem::separator()),
                item(cx, "Quit PhpCraft", "phpcraft::Quit"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        },
        menu(
            cx,
            "File",
            &[
                Some(("New Project…", "workspace::NewProject")),
                Some(("Open…", "workspace::OpenFolder")),
                Some(("Clone Repository…", "workspace::OpenProjects")),
                Some(("Recent Projects…", "workspace::OpenProjects")),
                Some(("Close Project", "workspace::CloseProject")),
                None,
                Some(("Save All", "workspace::SaveAll")),
                Some(("Reload All from Disk", "workspace::ReloadAll")),
                Some(("Local History", "workspace::ShowLocalHistory")),
                Some(("Reindex Project", "workspace::Reindex")),
                None,
                Some(("Settings…", "workspace::OpenSettings")),
                Some(("Settings Sync: Push", "workspace::SyncPush")),
                Some(("Settings Sync: Pull", "workspace::SyncPull")),
            ],
        ),
        menu(
            cx,
            "Edit",
            &[
                Some(("Undo", "editor::Undo")),
                Some(("Redo", "editor::Redo")),
                None,
                Some(("Cut", "editor::Cut")),
                Some(("Copy", "editor::Copy")),
                Some(("Paste", "editor::Paste")),
                Some(("Select All", "editor::SelectAll")),
                None,
                Some(("Find…", "editor::ShowFind")),
                Some(("Replace…", "editor::ShowReplace")),
                Some(("Find in Files…", "workspace::FindInFiles")),
                Some(("Replace in Files…", "workspace::ReplaceInFiles")),
                None,
                Some(("Duplicate Line", "editor::DuplicateLine")),
                Some(("Delete Line", "editor::DeleteLine")),
                Some(("Move Line Up", "editor::MoveLineUp")),
                Some(("Move Line Down", "editor::MoveLineDown")),
            ],
        ),
        menu(
            cx,
            "View",
            &[
                Some(("Project", "workspace::ShowProject")),
                Some(("Structure", "workspace::ToggleStructure")),
                Some(("Commit", "workspace::ShowCommit")),
                Some(("Git", "workspace::ToggleGitLog")),
                Some(("Terminal", "workspace::ToggleTerminal")),
                Some(("AI Assistant", "workspace::ToggleAgent")),
                None,
                Some(("Markdown Preview", "editor::ToggleMarkdownPreview")),
                Some(("Git Blame", "editor::ToggleBlame")),
            ],
        ),
        menu(
            cx,
            "Navigate",
            &[
                Some(("Search Everywhere", "workspace::SearchEverywhere")),
                Some(("Class…", "workspace::GoToClass")),
                Some(("File…", "workspace::GoToFile")),
                Some(("Symbol…", "workspace::GoToSymbol")),
                Some(("Line…", "workspace::GoToLine")),
                Some(("Recent Files…", "workspace::RecentFiles")),
                None,
                Some(("Declaration", "editor::GoToDeclaration")),
                Some(("Back", "workspace::NavigateBack")),
                None,
                Some(("Next Tab", "workspace::NextTab")),
                Some(("Previous Tab", "workspace::PrevTab")),
            ],
        ),
        menu(
            cx,
            "Code",
            &[
                Some(("Code Completion", "editor::ShowCompletions")),
                Some(("Show Intention Actions", "editor::ShowIntentions")),
                Some(("Optimize Imports", "editor::OptimizeImports")),
                Some(("Comment with Line Comment", "editor::ToggleComment")),
            ],
        ),
        menu(
            cx,
            "Laravel",
            &[
                Some(("Code Generation…", "workspace::OpenMake")),
                Some(("New Eloquent Model…", "workspace::NewModel")),
                Some(("New Eloquent Model from Database Table…", "workspace::ModelFromTable")),
                Some(("New Pivot Table…", "workspace::NewPivotTable")),
                None,
                Some(("Route Search…", "workspace::OpenRouteSearch")),
                Some(("Run Artisan Command…", "workspace::OpenArtisan")),
                None,
                Some(("Generate Helper Code", "workspace::GenerateHelperCode")),
                Some(("Remove Model PhpDocs…", "workspace::RemoveModelPhpDocs")),
                Some(("Find Unused Views", "workspace::FindUnusedViews")),
                None,
                Some(("Laravel Settings…", "workspace::OpenLaravelSettings")),
            ],
        ),
        menu(
            cx,
            "Run",
            &[
                Some(("Run…", "workspace::RunPopup")),
                Some(("Rerun", "workspace::RunLast")),
                Some(("Run Context Configuration", "workspace::RunContext")),
                Some(("Stop", "workspace::StopRun")),
                None,
                Some(("Edit Configurations…", "workspace::EditRunConfigurations")),
            ],
        ),
        menu(
            cx,
            "Tools",
            &[
                Some(("Deployment: Upload to Server", "workspace::UploadToServer")),
                Some(("Deployment: Download from Server", "workspace::DownloadFromServer")),
            ],
        ),
        menu(
            cx,
            "Git",
            &[
                Some(("Commit…", "workspace::ShowCommit")),
                Some(("Commit Changes", "workspace::CommitChanges")),
                Some(("Commit and Push", "workspace::CommitAndPush")),
                Some(("Push…", "workspace::GitPush")),
                Some(("Update Project…", "workspace::GitPull")),
                None,
                Some(("Branches…", "workspace::OpenBranches")),
                Some(("Show Git Log", "workspace::ToggleGitLog")),
            ],
        ),
        menu(cx, "Help", &[Some(("About PhpCraft", "workspace::About"))]),
    ];
    cx.set_menus(menus);
}
