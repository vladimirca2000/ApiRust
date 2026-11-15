#!/usr/bin/env bash
#
# Verifica a documentação de variáveis locais, que `rustc` e `clippy` não
# conseguem cobrir: o Rust não permite doc comment (`///`) em `let`.
#
# Política aplicada (somente em código de produção, fora de `#[cfg(test)]`):
#
#   1. Nome de variável com 1–2 caracteres ou genérico (`value`, `data`, `tmp`…)
#      precisa de um comentário `//` na linha imediatamente acima.
#   2. Literal numérico "mágico" (fora de 0, 1, 2, 0.0, 1.0) precisa de
#      comentário acima ou de vir de uma constante nomeada.
#
# Nomes descritivos (`foreign_keys`, `jwt_access_ttl_secs`) são aprovados sem
# comentário: repetir o nome em texto seria ruído, não documentação.
#
# Uso:
#   scripts/check-local-docs.sh
#
# Códigos de saída: 0 = sem violações, 1 = violações encontradas.

set -uo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Nomes que não comunicam intenção por si só.
readonly OPAQUE_NAMES='x|y|z|i|j|k|n|s|v|a|b|e|f|tmp|temp|val|value|data|res|aux|foo|bar|baz|stuff|thing|obj|item|ret|out|buf|str|num|idx|pos|len|max|min|cur|prev|next|arg|args|opt|opts'

readonly VIOLATIONS_FILE="$(mktemp)"
trap 'rm -f "${VIOLATIONS_FILE}"' EXIT

# Número que não faz parte de identificador, versão de string nem comentário.
readonly MAGIC_NUMBER_RE='(^|[^A-Za-z0-9_."])[[:space:]]*([0-9]+(\.[0-9]+)?)'

# Percorre cada arquivo de produção e valida as regras linha a linha.
check_file() {
    local file="$1"
    local line_number=0
    local in_tests=0
    # Profundidade de parênteses dentro de um bloco `#[...]` multilinha.
    local attr_depth=0

    while IFS= read -r line; do
        line_number=$((line_number + 1))

        # A partir do módulo de testes, só o próprio atributo é avaliado.
        if [[ "$line" =~ ^[[:space:]]*#\[cfg\(test\)\] ]]; then
            in_tests=1
            continue
        fi
        [[ "$in_tests" -eq 1 ]] && continue

        # --- Rastreamento de blocos de atributo multilinha ---
        # Em `#[utoipa::path(...)]` os números são códigos de status
        # documentados, não lógica. O bloco pode ocupar dezenas de linhas, então
        # acompanhamos a profundidade de parênteses até ele fechar.
        if [[ "$attr_depth" -gt 0 ]]; then
            attr_depth=$((attr_depth + $(count_char "$line" '(') - $(count_char "$line" ')')))
            [[ "$attr_depth" -lt 0 ]] && attr_depth=0
            continue
        fi
        if [[ "$line" =~ ^[[:space:]]*#\[ ]]; then
            attr_depth=$(( $(count_char "$line" '(') - $(count_char "$line" ')') ))
            [[ "$attr_depth" -lt 0 ]] && attr_depth=0
            continue
        fi

        # --- Regra 1: declaração `let` com nome opaco ---
        if [[ "$line" =~ ^[[:space:]]*let[[:space:]]+(mut[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*) ]]; then
            local name="${BASH_REMATCH[2]}"
            if [[ "$name" =~ ^(${OPAQUE_NAMES})$ ]]; then
                if ! has_comment_above "$file" "$line_number"; then
                    printf '%s:%d: nome-opaco: variável `%s` precisa de comentário acima\n' \
                        "${file#"${ROOT_DIR}/"}" "$line_number" "$name" >> "$VIOLATIONS_FILE"
                fi
            fi
        fi

        # --- Regra 2: literal numérico mágico ---
        # Constantes e `static` existem justamente para nomear números.
        # Comentários são ignorados: `// RFC 3339` não é número mágico.
        # Atributos (`#[...]`) também: em `#[utoipa::path(...)]` os números são
        # códigos de status documentados, não lógica executável.
        if [[ ! "$line" =~ ^[[:space:]]*(pub[[:space:]]+)?(const|static)[[:space:]] ]]; then
            if [[ ! "$line" =~ ^[[:space:]]*// ]] && [[ ! "$line" =~ ^[[:space:]]*#\[ ]]; then
                if [[ "$line" =~ $MAGIC_NUMBER_RE ]]; then
                    local literal="${BASH_REMATCH[2]}"
                    case "$literal" in
                        0|1|2|0.0|1.0) ;;
                        *)
                            if ! has_comment_above "$file" "$line_number"; then
                                printf '%s:%d: numero-magico: literal `%s` precisa de comentário ou constante nomeada\n' \
                                    "${file#"${ROOT_DIR}/"}" "$line_number" "$literal" >> "$VIOLATIONS_FILE"
                            fi
                            ;;
                    esac
                fi
            fi
        fi
    done < "$file"
}

# Indica se há comentário `//` na linha imediatamente acima da informada.
has_comment_above() {
    local file="$1" line="$2"
    local previous=$((line - 1))
    [[ "$previous" -lt 1 ]] && return 1
    sed -n "${previous}p" "$file" | grep -qE '^[[:space:]]*//'
}

# Conta quantas vezes um caractere aparece na linha.
count_char() {
    local text="$1" char="$2"
    local stripped="${text//[$char]/}"
    echo $(( ${#text} - ${#stripped} ))
}

# Coleta apenas arquivos de código de produção (exclui `tests/` de integração).
# `page.rs` é HTML/CSS embutido numa string: números ali são estilos, não lógica.
while IFS= read -r file; do
    case "$(basename "$file")" in
        page.rs) continue ;;
    esac
    check_file "$file"
done < <(
    find "${ROOT_DIR}/crates" -type f -name '*.rs' -path '*/src/*' | sort
)

violations=$(wc -l < "$VIOLATIONS_FILE" | tr -d ' ')

if [[ "$violations" -gt 0 ]]; then
    cat "$VIOLATIONS_FILE"
    printf '\n%d violação(ões) de documentação local.\n' "$violations"
    exit 1
fi

printf 'Documentação de variáveis locais: ok\n'
