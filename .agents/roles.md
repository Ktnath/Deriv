# Antigravity Multi-Agent Roles & Governance

This document defines the specialized roles used in the autonomous development loop.

## 1. Chef de Projet (PM / Orchestrateur)
- **Mission**: Transform requirements into testable deliverables.
- **Livrables**: `docs/PRD.md`, `docs/ACCEPTANCE.md`.
- **Gate**: All acceptance criteria = PASS.

## 2. Architecte / Tech Lead
- **Mission**: Technical structure and high-level design.
- **Livrables**: `docs/ARCHITECTURE.md`, `docs/ADR/*.md`.
- **Gate**: Architecture validated by PM.

## 3. Programmeur (Dev)
- **Mission**: Implementation of features and tests.
- **Livrables**: Code, `README.md`, `docs/RUNBOOK.md`.
- **Gate**: Build + Tests + Lint = PASS.

## 4. QA / Quality Gate
- **Mission**: Factual verification of requirements.
- **Livrables**: Updates to `docs/CHANGE_REQUESTS.md`.
- **Gate**: `STATUS: APPROVED` (0 blocker/major).

## 5. DevOps / Release
- **Mission**: Deployment and reproducibility.
- **Livrables**: `Dockerfile`, `compose.yml`, `docs/DEPLOYMENT.md`.
- **Gate**: Green pipeline.

## 6. Sécurité
- **Mission**: Risk reduction and secret management.
- **Livrables**: `docs/THREAT_MODEL.md`.
- **Gate**: No exposed secrets, surface mastered.

## 7. Documentation / UX
- **Mission**: Clarity and usability.
- **Livrables**: `docs/USER_GUIDE.md`.
- **Gate**: Tasks are documented for non-tech users.

## 8. Data & Backtest Analyst (Analyste de Données)
- **Mission**: Logic, traceability, and strategy validation through backtesting.
- **Livrables**: `docs/DATA_DICTIONARY.md`, `docs/METRICS.md`, `backtest/results/*.json`.
- **Gate**: Strategy matches historical performance expectations.

## 9. Ethics, Risk & Compliance (Éthique et Risques)
- **Mission**: Compliance, risk reduction, and protocol adherence.
- **Livrables**: `docs/ETHICS_REVIEW.md`, `docs/RISK_ASSESSMENT.md`.
- **Gate**: Risks mitigated and usage framed.
