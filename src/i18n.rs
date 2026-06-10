use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Pt,
    En,
}

static LANG: OnceLock<Lang> = OnceLock::new();

/// Define o idioma uma única vez no startup (mudança em runtime exige reiniciar).
pub fn set_lang(l: Lang) {
    let _ = LANG.set(l);
}

pub fn lang() -> Lang {
    *LANG.get().unwrap_or(&Lang::Pt)
}

pub fn from_code(s: &str) -> Lang {
    if s.starts_with("en") {
        Lang::En
    } else {
        Lang::Pt
    }
}

pub fn code(l: Lang) -> &'static str {
    match l {
        Lang::Pt => "pt",
        Lang::En => "en",
    }
}

/// Tradução de uma chave para o idioma atual.
pub fn tr(key: &str) -> &'static str {
    match lang() {
        Lang::En => en(key),
        Lang::Pt => pt(key),
    }
}

fn pt(key: &str) -> &'static str {
    match key {
        // app / header
        "open_new_window_tooltip" => "Abrir XML em nova janela",
        "new_file_tooltip" => "Novo arquivo em branco",
        "app_subtitle" => "Leitor de XML em tabela",
        "open_file" => "Abrir arquivo",
        "no_file_open" => "Nenhum arquivo aberto",
        "no_file_desc" => "Abra um arquivo .xml para visualizar em tabela.",
        "open_xml_title" => "Abrir XML",
        "xml_files" => "Arquivos XML",
        // settings
        "settings" => "Configurações",
        "general" => "Geral",
        "language" => "Idioma",
        "theme" => "Tema",
        "color_scheme" => "Esquema de cores",
        "theme_system" => "Seguir o sistema",
        "theme_light" => "Claro",
        "theme_dark" => "Escuro",
        "appearance" => "Aparência",
        "appearance_auto" => "Automático",
        "about" => "Sobre",
        "about_desc" => "Leitor e editor de arquivos XML em formato de tabela, nativo para Linux (GTK4 + libadwaita).",
        "author" => "Autor",
        "repository" => "Repositório",
        "lang_restart_note" => "O idioma será aplicado ao reiniciar o app.",
        // close
        "unsaved_title" => "Alterações não salvas",
        "unsaved_body" => "Você fez edições que ainda não foram salvas. O que deseja fazer?",
        "cancel" => "Cancelar",
        "close_without_saving" => "Fechar sem salvar",
        "save_ellipsis" => "Salvar…",
        "ok" => "OK",
        // document toolbar
        "find_placeholder" => "Localizar…",
        "filter_placeholder" => "Filtrar (SQL WHERE) — ex: valor <> '0.00'",
        "filter_tooltip" => "Aplicar filtro (WHERE)",
        "sql_tooltip" => "Consulta SQL completa (somente leitura)",
        "sum_label" => "Σ Somar",
        "sum_tooltip" => "Somar uma coluna",
        "columns_label" => "Colunas…",
        "columns_tooltip" => "Adicionar, renomear ou excluir colunas",
        "add_row_label" => "+ Linha",
        "add_row_tooltip" => "Adicionar uma linha em branco",
        "save_tooltip" => "Salvar como .xml",
        "csv_tooltip" => "Exportar CSV",
        // dialogs
        "sum_column_title" => "Somar coluna",
        "sum_column_body" => "Escolha a coluna a somar:",
        "sum" => "Somar",
        "total" => "Total",
        "numeric_values" => "valor(es) numérico(s)",
        "sum_failed" => "Não foi possível somar:",
        "sql_title" => "Consulta SQL",
        "sql_body" => "Tabela: dados — resultado somente leitura.",
        "run" => "Executar",
        "error" => "Erro",
        "save_as_xml" => "Salvar como .xml",
        "export_csv_title" => "Exportar CSV",
        "csv_export_error" => "Erro ao exportar CSV:",
        "save_error" => "Erro ao salvar:",
        "cell_write_error" => "Erro ao gravar célula:",
        "filter_error" => "Erro no filtro SQL:",
        "sql_error" => "Erro SQL:",
        // columns dialog
        "manage_columns" => "Gerenciar colunas",
        "manage_columns_body" => "Adicione, renomeie ou exclua colunas do documento.",
        "new_column_placeholder" => "Nome da nova coluna",
        "add" => "Adicionar",
        "delete" => "Excluir",
        "close" => "Fechar",
        "column_op_error" => "Erro na operação de coluna:",
        // menu de contexto (botão direito)
        "ctx_row_above" => "Nova linha acima",
        "ctx_row_below" => "Nova linha abaixo",
        "ctx_row_delete" => "Excluir linha",
        "ctx_col_add" => "Adicionar coluna",
        "ctx_col_delete" => "Excluir coluna",
        "ctx_col_sum" => "Σ Somar coluna",
        "rename_column" => "Renomear coluna",
        // text mode
        "text_mode_status" => "Modo texto — XML sem estrutura de tabela",
        "save_as" => "Salvar como…",
        _ => "",
    }
}

fn en(key: &str) -> &'static str {
    match key {
        "open_new_window_tooltip" => "Open XML in a new window",
        "new_file_tooltip" => "New blank file",
        "app_subtitle" => "XML table reader",
        "open_file" => "Open file",
        "no_file_open" => "No file open",
        "no_file_desc" => "Open an .xml file to view it as a table.",
        "open_xml_title" => "Open XML",
        "xml_files" => "XML files",
        "settings" => "Settings",
        "general" => "General",
        "language" => "Language",
        "theme" => "Theme",
        "color_scheme" => "Color scheme",
        "theme_system" => "Follow system",
        "theme_light" => "Light",
        "theme_dark" => "Dark",
        "appearance" => "Appearance",
        "appearance_auto" => "Automatic",
        "about" => "About",
        "about_desc" => "Reader and editor for XML table files, native to Linux (GTK4 + libadwaita).",
        "author" => "Author",
        "repository" => "Repository",
        "lang_restart_note" => "The language will apply after restarting the app.",
        "unsaved_title" => "Unsaved changes",
        "unsaved_body" => "You made edits that have not been saved yet. What would you like to do?",
        "cancel" => "Cancel",
        "close_without_saving" => "Close without saving",
        "save_ellipsis" => "Save…",
        "ok" => "OK",
        "find_placeholder" => "Find…",
        "filter_placeholder" => "Filter (SQL WHERE) — e.g. valor <> '0.00'",
        "filter_tooltip" => "Apply filter (WHERE)",
        "sql_tooltip" => "Full SQL query (read-only)",
        "sum_label" => "Σ Sum",
        "sum_tooltip" => "Sum a column",
        "columns_label" => "Columns…",
        "columns_tooltip" => "Add, rename or delete columns",
        "add_row_label" => "+ Row",
        "add_row_tooltip" => "Add a blank row",
        "save_tooltip" => "Save as .xml",
        "csv_tooltip" => "Export CSV",
        "sum_column_title" => "Sum column",
        "sum_column_body" => "Choose the column to sum:",
        "sum" => "Sum",
        "total" => "Total",
        "numeric_values" => "numeric value(s)",
        "sum_failed" => "Could not sum:",
        "sql_title" => "SQL query",
        "sql_body" => "Table: dados — read-only result.",
        "run" => "Run",
        "error" => "Error",
        "save_as_xml" => "Save as .xml",
        "export_csv_title" => "Export CSV",
        "csv_export_error" => "Error exporting CSV:",
        "save_error" => "Error saving:",
        "cell_write_error" => "Error writing cell:",
        "filter_error" => "SQL filter error:",
        "sql_error" => "SQL error:",
        "manage_columns" => "Manage columns",
        "manage_columns_body" => "Add, rename or delete the document's columns.",
        "new_column_placeholder" => "New column name",
        "add" => "Add",
        "delete" => "Delete",
        "close" => "Close",
        "column_op_error" => "Column operation error:",
        "ctx_row_above" => "New row above",
        "ctx_row_below" => "New row below",
        "ctx_row_delete" => "Delete row",
        "ctx_col_add" => "Add column",
        "ctx_col_delete" => "Delete column",
        "ctx_col_sum" => "Σ Sum column",
        "rename_column" => "Rename column",
        "text_mode_status" => "Text mode — XML without table structure",
        "save_as" => "Save as…",
        _ => pt(key),
    }
}
