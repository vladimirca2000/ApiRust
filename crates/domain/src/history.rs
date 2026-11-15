//! Histórico do catálogo: movimentação de estoque e mudança de preço.
//!
//! O produto guarda apenas o **saldo atual** de estoque e o **preço atual**.
//! Sem essas entidades, qualquer pergunta do tipo "por que o saldo caiu?" ou
//! "por quanto vendíamos em março?" fica sem resposta: o `UPDATE` sobrescreve
//! o valor anterior e a informação se perde.

use chrono::{DateTime, Utc};

use crate::error::DomainError;
use crate::user_id::UserId;

/// Motivo registrado para uma variação de estoque.
///
/// O motivo é o que separa "vendi" de "perdi" de "corrigi o inventário": sem
/// ele, um relatório de perdas é impossível mesmo com o histórico completo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockReason {
    /// Saldo inicial informado na criação do produto.
    Initial,
    /// Ajuste manual feito por um operador.
    Correction,
    /// Venda ao cliente.
    Sale,
    /// Perda, avaria ou vencimento.
    Loss,
    /// Reposição de compra.
    Restock,
}

impl StockReason {
    /// Devolve o texto persistido no banco (`snake_case`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Initial => "initial",
            Self::Correction => "correction",
            Self::Sale => "sale",
            Self::Loss => "loss",
            Self::Restock => "restock",
        }
    }

    /// Converte o texto persistido de volta no motivo.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::InvalidStockReason`] para texto fora do conjunto
    /// conhecido — melhor falhar alto do que tratar dado corrompido como
    /// `correction`.
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        match value {
            "initial" => Ok(Self::Initial),
            "correction" => Ok(Self::Correction),
            "sale" => Ok(Self::Sale),
            "loss" => Ok(Self::Loss),
            "restock" => Ok(Self::Restock),
            other => Err(DomainError::InvalidStockReason(other.to_owned())),
        }
    }
}

/// Uma variação registrada de estoque de um produto.
///
/// `delta` é a **diferença** aplicada ao saldo (negativa quando sai estoque),
/// e não o saldo final: só a diferença permite reconstruir o saldo somando a
/// série, independentemente de quantos ajustes ocorreram no meio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StockMovement {
    /// Identificador do produto afetado.
    pub product_id: UserId,
    /// Variação aplicada ao saldo (nunca zero).
    pub delta: i64,
    /// Motivo da variação.
    pub reason: StockReason,
    /// Quem executou a operação, quando houve ator autenticado.
    pub actor_id: Option<UserId>,
    /// Instante da operação, em UTC.
    pub at: DateTime<Utc>,
}

impl StockMovement {
    /// Cria uma movimentação, recusando variação nula.
    ///
    /// Delta zero não é erro de digitação inofensivo: seria uma linha no
    /// histórico que não explica nenhuma mudança de saldo.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::EmptyStockMovement`] quando `delta` é zero.
    pub fn new(
        product_id: UserId,
        delta: i64,
        reason: StockReason,
        actor_id: Option<UserId>,
        at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        if delta == 0 {
            return Err(DomainError::EmptyStockMovement);
        }

        Ok(Self {
            product_id,
            delta,
            reason,
            actor_id,
            at,
        })
    }
}

/// Uma mudança registrada de preço de um produto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceChange {
    /// Identificador do produto afetado.
    pub product_id: UserId,
    /// Preço anterior, em centavos.
    pub old_cents: i64,
    /// Preço novo, em centavos.
    pub new_cents: i64,
    /// Quem executou a alteração, quando houve ator autenticado.
    pub actor_id: Option<UserId>,
    /// Instante da alteração, em UTC.
    pub at: DateTime<Utc>,
}

impl PriceChange {
    /// Cria o registro apenas quando o preço realmente mudou.
    ///
    /// # Errors
    ///
    /// Retorna [`DomainError::UnchangedPrice`] quando os valores são iguais —
    /// gravar um "histórico" sem mudança polui a série e quebra relatórios que
    /// contam alterações.
    pub fn new(
        product_id: UserId,
        old_cents: i64,
        new_cents: i64,
        actor_id: Option<UserId>,
        at: DateTime<Utc>,
    ) -> Result<Self, DomainError> {
        if old_cents == new_cents {
            return Err(DomainError::UnchangedPrice);
        }

        Ok(Self {
            product_id,
            old_cents,
            new_cents,
            actor_id,
            at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Instante fixo usado nas asserções.
    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 15, 12, 0, 0)
            .single()
            .expect("instante válido")
    }

    /// Identificador de produto usado nos testes.
    fn product() -> UserId {
        UserId::new_v4()
    }

    /// Verifica o contrato identificado por `stock_reason_round_trips_through_text` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn stock_reason_round_trips_through_text() {
        for reason in [
            StockReason::Initial,
            StockReason::Correction,
            StockReason::Sale,
            StockReason::Loss,
            StockReason::Restock,
        ] {
            assert_eq!(
                StockReason::parse(reason.as_str()).expect("motivo conhecido"),
                reason
            );
        }
    }

    /// Verifica o contrato identificado por `stock_reason_rejects_unknown_text` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn stock_reason_rejects_unknown_text() {
        assert!(matches!(
            StockReason::parse("roubo"),
            Err(DomainError::InvalidStockReason(_))
        ));
    }

    /// Verifica o contrato identificado por `stock_movement_accepts_negative_and_positive_delta` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn stock_movement_accepts_negative_and_positive_delta() {
        let entry = StockMovement::new(product(), -3, StockReason::Sale, None, now())
            .expect("delta negativo é válido");
        assert_eq!(entry.delta, -3);

        let entry = StockMovement::new(product(), 10, StockReason::Restock, None, now())
            .expect("delta positivo é válido");
        assert_eq!(entry.delta, 10);
    }

    /// Verifica o contrato identificado por `stock_movement_rejects_zero_delta` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn stock_movement_rejects_zero_delta() {
        assert!(matches!(
            StockMovement::new(product(), 0, StockReason::Correction, None, now()),
            Err(DomainError::EmptyStockMovement)
        ));
    }

    /// Verifica o contrato identificado por `price_change_requires_a_different_price` e protege o comportamento coberto pelas asserções.
    /// O teste retorna `()`; uma asserção incompatível reprova o cenário.
    #[test]
    fn price_change_requires_a_different_price() {
        let id = product();

        assert!(matches!(
            PriceChange::new(id, 1000, 1000, None, now()),
            Err(DomainError::UnchangedPrice)
        ));
        assert!(PriceChange::new(id, 1000, 1200, None, now()).is_ok());
    }
}
