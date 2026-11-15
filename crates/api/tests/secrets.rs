//! Garante que segredos locais não entram no repositório.
//!
//! Aceite da Fase 7: `.env` e o banco ficam fora do Git; `.env.example` só
//! contém placeholders. Um segredo versionado por engano é falha de segurança,
//! não de estilo — por isso o teste lê os arquivos reais do workspace.

use std::path::Path;
use std::process::Command;

/// Raiz do workspace (dois níveis acima de `crates/api`).
fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/api está dois níveis abaixo da raiz")
}

/// Confirma que o `.gitignore` cobre `.env`, variantes e o diretório de dados.
#[test]
fn gitignore_covers_secrets_and_local_data() {
    let gitignore =
        std::fs::read_to_string(workspace_root().join(".gitignore")).expect("ler .gitignore");

    assert!(
        gitignore.lines().any(|line| line.trim() == ".env"),
        ".gitignore deve ignorar `.env`"
    );
    assert!(
        gitignore.contains(".env*"),
        ".gitignore deve cobrir variantes como `.env.local`"
    );
    assert!(
        gitignore.lines().any(|line| line.trim() == "data/"),
        ".gitignore deve ignorar o banco local em `data/`"
    );
}

/// Confirma que o exemplo versionado não carrega senha real nem JWT de produção.
#[test]
fn env_example_contains_only_placeholders() {
    let example =
        std::fs::read_to_string(workspace_root().join(".env.example")).expect("ler .env.example");

    assert!(
        example.contains("troque-este-segredo"),
        "JWT_SECRET do exemplo deve ser um placeholder explícito"
    );

    for line in example.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }
        assert!(
            !trimmed.starts_with("ADMIN_PASSWORD="),
            "ADMIN_PASSWORD não pode ir descomentado no exemplo versionado"
        );
        assert!(
            !trimmed.starts_with("ADMIN_EMAIL="),
            "ADMIN_EMAIL não pode ir descomentado no exemplo versionado"
        );
    }

    // REDIS_URL vazio (ou ausente de valor) no exemplo: a senha real fica só no `.env`.
    let redis_line = example
        .lines()
        .find(|line| line.starts_with("REDIS_URL="))
        .expect("REDIS_URL documentado no exemplo");
    assert_eq!(
        redis_line.trim(),
        "REDIS_URL=",
        "exemplo não pode embutir credencial de Redis"
    );
}

/// Confirma que o Git não rastreia `.env` (arquivo de segredos reais).
#[test]
fn env_file_is_not_tracked_by_git() {
    let output = Command::new("git")
        .args(["ls-files", "--error-unmatch", ".env"])
        .current_dir(workspace_root())
        .output()
        .expect("git ls-files");

    assert!(
        !output.status.success(),
        ".env está rastreado pelo Git — remova-o do índice e confirme o .gitignore"
    );
}
