# dendriForge — ROADMAP

> **Filosofia do projeto:** Isolar lógica no Core (Rust) e usar o Svelte exclusivamente para renderização.
> Nenhuma feature avança sem testes aprovados. Perfeição antes de velocidade.
>
> *Este documento foi reescrito em Junho 2026 a partir da leitura directa de todos os ficheiros do repositório.*
> *Repositório: [caiojordao84/dendriForge](https://github.com/caiojordao84/dendriForge)*
> *Spec TOON: [toonformat.dev/reference/spec](https://toonformat.dev/reference/spec)*

---

## Índice

1. [Visão Geral e Princípios de Engenharia](#1-visão-geral-e-princípios-de-engenharia)
2. [Stack Técnica Canónica](#2-stack-técnica-canónica)
3. [Estrutura do Repositório](#3-estrutura-do-repositório)
4. [Contratos Core-UI](#4-contratos-core-ui)
5. [Modelo de Projeto Interno](#5-modelo-de-projeto-interno)
6. [Estado Actual do Core de Dados](#6-estado-actual-do-core-de-dados)
7. [M0 — Limpeza Imediata (pré-Fase 0)](#m0--limpeza-imediata-pré-fase-0)
8. [Fase 0 — Fundação e Contratos](#fase-0--fundação-e-contratos)
9. [Fase 1 — Parser (Código → ASL)](#fase-1--parser-código--asl)
10. [Fase 2 — Generator (ASL → Código)](#fase-2--generator-asl--código)
11. [Fase 3 — UI Barebones e Shell Global](#fase-3--ui-barebones-e-shell-global)
12. [Fase 4 — Janela de Simulação](#fase-4--janela-de-simulação)
13. [Fase 5 — Editor GRAFCET](#fase-5--editor-grafcet)
14. [Fase 6 — Editor Ladder](#fase-6--editor-ladder)
15. [Fase 7 — Editor HMI, SCADA e IoT](#fase-7--editor-hmi-scada-e-iot)
16. [Fase 8 — Persistência e Segurança](#fase-8--persistência-e-segurança)
17. [Fase 9 — Testes, Hardening e Documentação](#fase-9--testes-hardening-e-documentação)
18. [Critérios de Pronto e Quality Gates](#critérios-de-pronto-e-quality-gates)
19. [Escopo da Versão 0.1.0](#escopo-da-versão-010)
20. [Expansão pós 0.1.0](#expansão-pós-010)
21. [Regras de Engenharia Imutáveis](#regras-de-engenharia-imutáveis)

---

## 1. Visão Geral e Princípios de Engenharia

O dendriForge é uma plataforma integrada de desenvolvimento e simulação industrial composta por cinco módulos fundamentais: Simulação, Transpilação/Parser, GRAFCET, Ladder e HMI/SCADA/IoT. O núcleo semântico de toda a plataforma é a linguagem ASL (Abstract Syntax Language), estruturada sobre a especificação [TOON v3.0 (Token-Oriented Object Notation)](https://toonformat.dev/reference/spec).

O sistema tem **três camadas independentes** que são desenvolvidas sequencialmente — cada camada é consumida pela seguinte:

| Camada | O que faz | Estado actual |
|---|---|---|
| **Core de dados** | Profiles de boards, componentes, estilos, exemplos ASL | ✅ Activo — dados existem |
| **Parser/Validator** | Lê e valida ficheiros `.toon` segundo `toon-dialect-core.md` | ❌ Não existe — spec pronta |
| **Engine/Linker** | Associa programas ASL a boards, valida roles, gera código | ❌ Não existe — spec parcial |

### Princípios Imutáveis

| Princípio | Descrição |
|---|---|
| **Core-first** | Toda lógica de domínio vive no Core Rust. O Svelte só renderiza projeções de estado. |
| **Contrato explícito** | A comunicação Core ↔ UI acontece exclusivamente por mensagens tipadas. |
| **Zero estado duplicado** | O estado canónico existe apenas no Core. A UI não mantém cópias de dados de domínio. |
| **Testes antes de avançar** | Nenhuma fase avança sem que os testes da fase anterior estejam verdes. |
| **TOON como fonte de verdade** | Boards, componentes, screens, programas e projetos são todos representados em `.toon`. |
| **Desktop first** | Experiência primária no desktop; mobile e web são contextos secundários. |

---

## 2. Stack Técnica Canónica

### Backend — Core Rust

| Camada | Tecnologia | Referência | Justificativa |
|---|---|---|---|
| Framework desktop | [Tauri v2](https://v2.tauri.app/concept/architecture/) | Rust + WebView nativo | Binários leves, sem Electron, bridge tipada com frontend |
| Async runtime | [tokio](https://github.com/tokio-rs/tokio) | `tokio = { version = "1", features = ["full"] }` | I/O não bloqueante, scheduler, timers para o clock de simulação |
| Serialização | [serde](https://serde.rs/) + [serde_json](https://github.com/nox/serde_json) | `serde = { features = ["derive"] }` | Serialização/deserialização canónica de mensagens e estado |
| Parser TOON/ASL | [winnow](https://docs.rs/winnow) | fork moderno do nom, zero-copy | Parser combinator performático para o dialeto TOON |
| Parser ST / MicroPython | [pest](https://github.com/pest-parser/pest) + pest_derive | PEG grammar declarativa | Gramáticas PEG de linguagens de terceiros — ST e MicroPython |
| Workspace Rust | [Cargo Workspace](https://reintech.io/blog/cargo-workspace-best-practices-large-rust-projects) | `[workspace]` em Cargo.toml raiz | Monorepo: múltiplos crates com Cargo.lock partilhado |

> **Nota sobre parsers:** `winnow` é preferido para o dialeto TOON/ASL (parsing incremental, zero-copy, streaming). `pest` com gramáticas PEG declarativas é preferido para ST e MicroPython, cujas gramáticas formais são públicas e estáveis. Os dois coexistem sem conflito no mesmo workspace.

### Frontend — Svelte + TypeScript

| Camada | Tecnologia | Referência | Justificativa |
|---|---|---|---|
| Framework UI | [Svelte 5](https://svelte.dev/) + TypeScript | Runes para estado reativo | Compilado, sem runtime, perfeito para UIs densas e performáticas |
| Estado global | [Svelte 5 Runes](https://svelte.dev/docs/svelte/what-are-runes) | `$state`, `$derived`, `$effect` | Runes são o sistema canónico de estado no Svelte 5 — stores apenas para casos específicos |
| Componentes UI | [shadcn-svelte](https://www.shadcn-svelte.com/docs/installation) | Cópia local, não dependência | Componentes acessíveis e customizáveis; CSS próprio do dendriForge mantido |
| Canvas 2D (simulação e HMI) | [Konva.js](https://konvajs.org/docs/index.html) | `npm install konva` | Renderização 2D de alta performance, SVG e shapes, drag-and-drop nativo |
| Canvas de grafos (GRAFCET e Ladder) | [Svelte Flow](https://svelteflow.dev/) | `npm install @xyflow/svelte` | Nós interativos, edges, handles — ideal para diagramas sequenciais e rungs |
| Editor de código | [CodeMirror 6](https://codemirror.net/) | `npm install codemirror` | Editor extensível com syntax highlighting para ST, MicroPython, C++, Rust |
| Tabelas dinâmicas (I/O) | [TanStack Table v8](https://tanstack.com/table/latest/docs/introduction) | Headless, framework-agnostic | Tabela de I/O global e mapeamentos de variáveis — headless garante estilo próprio |
| CSS base | [dendriForge-A11Y-Slate.css](https://github.com/caiojordao84/dendriForge/blob/main/core/styles/dendriForge-A11Y-Slate.css) | `core/styles/` do repositório | Sistema de design e tokens de simulação já validados (WCAG AAA) |
| Testes UI | [Vitest](https://vitest.dev/) + @testing-library/svelte | `npm install -D vitest` | Colocado junto ao código, Svelte 5 compatível |

---

## 3. Estrutura do Repositório

O repositório segue uma estrutura monorepo com Cargo Workspace para o backend e um projeto Svelte/Vite para o frontend, ambos coordenados pelo Tauri.

```
dendriForge/
├── Cargo.toml                ← Cargo Workspace raiz
├── Cargo.lock
├── package.json              ← Scripts do projeto (Tauri, Vite, testes)
├── tauri.conf.json           ← Configuração Tauri v2
├── ROADMAP.md
│
├── core/                     ← Dados canónicos (não tocam no app)
│   ├── asl/
│   │   └── v010/             ← ASL versão 0.1.0
│   │       ├── rules/        ← asl-program-profile.md (spec normativa)
│   │       └── examples/     ← 10 exemplos .toon (básico → avançado industrial)
│   ├── boards/               ← 47 perfis .toon de boards + 4 SVGs + rules/
│   │   └── rules/            ← asl-board-profile.md + toon-dialect-core.md
│   ├── components/           ← ~145 perfis .toon de componentes
│   │   └── rules/            ← ⚠️ VAZIO — falta asl-component-profile.md
│   ├── screens/              ← 2 perfis .toon HMI industriais (nascente)
│   └── styles/               ← dendriForge-A11Y-Slate.css
│
├── crates/                   ← Crates Rust do workspace
│   ├── df-model/             ← Tipos canónicos: ToonNode, AslProgram, Board, Component...
│   ├── df-parser/            ← Parser TOON/ASL (winnow) + ST + MicroPython (pest)
│   ├── df-sim/               ← Runtime de simulação, clock, scheduler
│   ├── df-codegen/           ← Generator ASL → ST, MicroPython, C++, Rust
│   ├── df-project/           ← Loading/saving de projetos, snapshots, migrações
│   └── df-bridge/            ← Comandos e eventos Tauri (ponte Core ↔ UI)
│
├── src/                      ← Frontend Svelte + TypeScript
│   ├── lib/
│   │   ├── core/             ← Stores Svelte que espelham estado do Core
│   │   ├── editors/          ← Componentes dos editores (simulação, grafcet, ladder, hmi)
│   │   ├── ui/               ← Componentes genéricos de UI (shadcn-svelte customizados)
│   │   └── utils/            ← Helpers de formatação, mapeamento, validação de UI
│   ├── routes/               ← Rotas Svelte
│   └── app.html
│
├── docs/                     ← Documentação movida de core/ (ver M0)
│   ├── boards/               ← comparison_table.md
│   └── components/           ← file_list.md
│
└── tests/                    ← Testes de integração e golden tests
    ├── golden/               ← Fixtures .toon de referência para round-trip
    └── integration/          ← Testes Core ↔ bridge
```

---

## 4. Contratos Core-UI

Toda comunicação entre Svelte e Rust usa o sistema de [comandos e eventos do Tauri v2](https://v2.tauri.app/concept/architecture/). O Frontend **nunca** muta estado canónico directamente — envia intenções; o Core responde com estado novo.

### Comandos (UI → Core)

```typescript
// Projeto
invoke('open_project', { path: string })
invoke('save_project', { path?: string })
invoke('new_project', { template: string })
invoke('validate_project')

// Parser
invoke('parse_source', { source: string, language: Language })
invoke('generate_code', { target: Language })

// Simulação
invoke('start_simulation')
invoke('pause_simulation')
invoke('stop_simulation')
invoke('step_simulation')        // Avançar um tick manual

// Modelo / Canvas
invoke('patch_node_props', { nodeId: string, props: ToonPatch })
invoke('add_board', { boardId: string, position: Vec2 })
invoke('add_component', { componentId: string, position: Vec2 })
invoke('remove_node', { nodeId: string })
invoke('add_wire', { from: PinRef, to: PinRef })
invoke('remove_wire', { wireId: string })

// I/O e Tags
invoke('declare_variable', { variable: IoVariable })
invoke('bind_tag', { elementId: string, tagName: string, property: string })

// Editor GRAFCET
invoke('add_grafcet_step', { props: GrafcetStep })
invoke('add_grafcet_transition', { props: GrafcetTransition })
invoke('update_grafcet_graph', { graph: GrafcetGraph })

// Editor Ladder
invoke('add_ladder_rung', { index?: number })
invoke('remove_ladder_rung', { index: number })
invoke('update_rung_element', { rungIndex: number, element: LadderElement })
```

### Eventos (Core → UI)

```typescript
// Estado do projeto
listen('project_loaded', (e: ProjectSnapshot) => {})
listen('project_saved', (e: { path: string }) => {})
listen('project_validation_result', (e: ValidationResult) => {})

// Parser / Codegen
listen('parse_result', (e: ParseResult) => {})
listen('generate_result', (e: GenerateResult) => {})
listen('diagnostic_report', (e: DiagnosticReport) => {})

// Simulação
listen('simulation_tick', (e: SimulationTickPayload) => {})
listen('runtime_state_changed', (e: RuntimeState) => {})
listen('pin_state_changed', (e: PinStateUpdate[]) => {})
listen('tag_value_changed', (e: TagUpdate[]) => {})

// Canvas
listen('layout_snapshot', (e: LayoutSnapshot) => {})
listen('binding_updated', (e: BindingUpdate) => {})

// Diagnósticos
listen('error', (e: AppError) => {})
listen('warning', (e: AppWarning) => {})
```

### Tipos Partilhados (TypeScript ↔ Rust via serde)

```typescript
type Language = 'structured-text' | 'micropython' | 'cpp' | 'rust-embassy' | 'ladder' | 'grafcet'
type RuntimeState = 'idle' | 'running' | 'paused' | 'fault'
type Vec2 = { x: number; y: number }
type PinRef = { nodeId: string; pinId: string }
```

---

## 5. Modelo de Projeto Interno

O ficheiro de projeto `.dforge` é um arquivo comprimido (ZIP) com estrutura interna previsível. Contém tudo o que é necessário para reabrir, simular e exportar um projeto sem dependências externas.

```
projeto.dforge  (ZIP)
├── manifest.toon      ← Metadados, versão ASL, boards/components referenciados
├── program.toon       ← Programa ASL principal (gerado ou editado)
├── layout.json        ← Estado visual do canvas de simulação
├── grafcet.json       ← Grafo GRAFCET (nós, transições, ações)
├── ladder.json        ← Array de rungs
├── hmi.json           ← Bindings e layout do editor HMI
├── io-table.toon      ← Tabela global de I/O e tags
├── sources/           ← Código-fonte textual por linguagem
│   ├── program.st
│   └── program.py
└── assets/            ← SVGs customizados importados pelo utilizador
```

---

## 6. Estado Actual do Core de Dados

> Esta secção reflecte o estado real do repositório em Junho 2026.
> Serve de baseline para as tarefas M0 e Fase 0.

### Inventário confirmado

| Pasta | Conteúdo actual | Gaps conhecidos |
|---|---|---|
| `core/boards/` | 47 `.toon` + 4 SVGs + `rules/` com 2 docs normativos | **43 boards sem SVG**; 1 artefacto `.toon.md` a remover |
| `core/components/` | ~145 `.toon` + 1 SVG | `rules/` **vazia** (falta spec); 2 scripts Python a remover; `file_list.md` a mover |
| `core/styles/` | `dendriForge-A11Y-Slate.css` (17 KB, WCAG AAA) | Variantes Dark, Light, High-Contrast em falta |
| `core/asl/v010/rules/` | `asl-program-profile.md` (15 KB) | — |
| `core/asl/v010/examples/` | 10 `.toon` (básico → industrial) + `LINGUAGEM-ASL-REFERENCIA.md` | `LINGUAGEM-ASL-REFERENCIA.md` está na pasta errada |
| `core/screens/` | 2 `.toon` HMI industriais (`ind-`) | Pasta nascente; sem variantes `maker-`; sem `rules/` |

### Estrutura normativa confirmada dos board profiles

Lida de `arduino-uno-r3.toon` — 7 secções canónicas obrigatórias:

```
§1  DEVICE IDENTIFICATION   → id, name, mcu, category, image, url
§2  TECH SPECS & DIMENSIONS → flash, sram, eeprom, clock, voltage, dims
§3  ELECTRICAL PROFILE      → powerPins[n|]{name|direction|voltage|type}
§4  GPIO MAP                → gpio[n|]{pin|type|pwm|int|label|roles}
§5  PERIPHERALS & USB       → peripherals[n|]{...}, usb{type|chip|vid|pid}
§6  RESTRICTIONS & COMPAT   → max_io_current, warnings[n], frameworks, bootloader
§7  AGENT SKILLS            → boardFamilySkillId, boardProfileId, defaultLanguageSkills
```

> **Nota crítica — §7 AGENT SKILLS:** cada board profile é um **contrato de capacidades para um agente AI**.
> O campo `boardFamilySkillId` (ex: `avr-family`) e `defaultLanguageSkills` (ex: `arduino-cpp-avr`, `rust-embassy-avr`)
> determinam quais os generators disponíveis para aquela família de hardware.
> O sistema de skills não está ainda documentado em `core/` — é o maior gap de especificação actual.

### Estrutura normativa confirmada dos programas ASL

Lida de `basic-blink-logic.toon` e `plc-elevator-advanced.toon` — 5 secções canónicas:

```
§1  HARDWARE INTERFACE      → hardware[n|]{pin|type|io_mode|label|target_role}
§2  SYSTEM DATA             → variáveis globais, constantes (TIME_UNIT, etc.)
§3  INITIALIZATION SYSTEM   → setup[n]: lista de chamadas imperativas
§4  BEHAVIORAL LOGIC (LOOP) → loop[n]: lista de funções a chamar em ciclo
§5  PROCEDURAL FUNCTIONS    → functions: sub-funções com lógica if/else indentada
```

> **Nota crítica — Resolver board↔ASL:** os programas ASL não referenciam boards directamente
> (usam apenas números de pino). O engine vai precisar de um **linker/resolver** que associe
> um programa ASL a um board profile, valide que os pinos existem, que os tipos são correctos,
> e que os `target_role` são compatíveis com os `roles` declarados no GPIO map.
> Esta peça não está especificada em nenhum ficheiro actual.

---

## M0 — Limpeza Imediata (pré-Fase 0)

> **Objectivo:** Eliminar artefactos incorrectos e mover documentação para os lugares certos.
> Sem dependências. Pode ser feito antes de qualquer linha de código Rust.

- [ ] Remover `core/boards/esp32-devkitc-v4.toon.md` (extensão errada — ficheiro de notas solto)
- [ ] Mover `core/boards/comparison_table.md` → `docs/boards/comparison_table.md`
- [ ] Mover `core/components/file_list.md` → `docs/components/file_list.md`
- [ ] Mover `core/asl/v010/examples/LINGUAGEM-ASL-REFERENCIA.md` → `core/asl/v010/rules/LINGUAGEM-ASL-REFERENCIA.md`
- [ ] Remover `core/components/resistor_color_code.py` (script Python sem lugar aqui)
- [ ] Remover `core/components/resistor_color_code.cpython-312.pyc` (bytecode compilado)
- [ ] Adicionar `.gitignore` com entradas `**/__pycache__/` e `**/*.pyc`
- [ ] Criar `docs/` na raiz com subdirectórios `boards/` e `components/`

---

## Fase 0 — Fundação e Contratos

> **Objectivo:** Estabelecer o workspace, os tipos canónicos, o loader TOON, o resolver board↔ASL e a bridge Tauri antes de qualquer UI.

### 0.0 — Especificação em falta (fazer antes de codificar)

- [ ] Criar `core/components/rules/asl-component-profile.md` — especificação normativa do formato de um profile de componente (análogo a `asl-board-profile.md`). Sem este documento, o loader de componentes não tem contrato.
- [ ] Criar `docs/architecture/board-asl-resolver.md` — especificação do resolver board↔ASL: como um programa ASL é associado a um board profile, validação de pinos, compatibilidade de roles, e erros de diagnóstico esperados.
- [ ] Documentar o sistema de `boardFamilySkillId` e `defaultLanguageSkills` — mapeamento de família de board para generators disponíveis.

### 0.1 — Cargo Workspace

- [ ] Criar `Cargo.toml` raiz com `[workspace]` declarando todos os crates em `crates/`
- [ ] Configurar `Cargo.lock` partilhado
- [ ] Criar crate `df-model` com todos os tipos Rust anotados com `#[derive(Serialize, Deserialize)]` via [serde](https://serde.rs/)
- [ ] Estrutura de tipos: `ToonNode`, `ToonDocument`, `AslProgram`, `BoardProfile`, `ComponentProfile`, `ScreenProfile`, `IoVariable`, `SimulationState`

### 0.2 — Loader TOON/ASL

O parser deve implementar a gramática completa de `core/boards/rules/toon-dialect-core.md` (TOON spec v3.0, 2025-11-24).

Regras normativas confirmadas a implementar:

- [ ] **§1** — Indentação: exactamente 2 espaços; tabs proibidos; sem trailing whitespace; LF ou CRLF aceites; sem newline final
- [ ] **§2** — Comentários: linhas com `#` como primeiro caractere não-whitespace são ignoradas; `#` inline proibido; `#FFFFFF` é valor, não comentário
- [ ] **§3** — Section headings: `## <index>. SECTION NAME IN UPPERCASE:` com índice incremental sem gaps
- [ ] **§4** — Keys: padrão `^[A-Za-z_][A-Za-z0-9_.]*$`; chaves com hífens ou espaços entre aspas duplas; chaves duplicadas no mesmo nível são erro fatal
- [ ] **§5** — Escalares: string (quoted/unquoted), integer, float, hex (`"0x..."` quoted), boolean (`true`/`false`), null, URL
- [ ] **§6** — Nested object blocks: chave terminada em `:` sem valor na mesma linha
- [ ] **§7** — Inline record delimiter: Space Pipe Space (` | `) para múltiplos `key: value` numa linha
- [ ] **§8** — Collections com size annotation: `name[n]:` — count declarado deve corresponder exactamente ao número de itens (erro fatal em strict mode)
- [ ] **§9** — Tabular arrays: `name[n|]{field1|field2|...}:` com pipe sem espaços nas linhas de dados
- [ ] **§10** — List arrays: prefixo `- ` com indentação de 2 espaços; multi-field com pipe sem espaços
- [ ] **§11** — Sub-value separator: `;` sem espaços envolventes (extensão de dialecto — tratada antes do parser TOON base)
- [ ] **§12** — Quoting: apenas aspas duplas; escapes: `\\`, `\"`, `\n`, `\r`, `\t`; multi-line strings proibidas
- [ ] **§13** — Bloco METADATA: obrigatório no topo; campos obrigatórios: `project_name`, `version`, `editor`, `author`, `ASLversion`; `ASLversion` em semver `MAJOR.MINOR.PATCH`
- [ ] **§15** — Documento: UTF-8 sem BOM; extensão `.toon`; root é objecto implícito; strict mode por defeito
- [ ] Dois modos: **strict** (erros fatais para mismatches de count, delimiters, escapes desconhecidos) e **lenient** (avisos)
- [ ] Loader de boards: lê todos os `.toon` em `core/boards/` → `Vec<BoardProfile>`
- [ ] Loader de components: lê todos os `.toon` em `core/components/` → `Vec<ComponentProfile>`
- [ ] Loader de screens: lê todos os `.toon` em `core/screens/` → `Vec<ScreenProfile>`
- [ ] Round-trip test: parse → serialize → reparse deve produzir estrutura idêntica
- [ ] **Quality Gate:** 100% de coverage nos golden tests de boards e components existentes

### 0.3 — Resolver Board↔ASL

> Esta peça não existia no ROADMAP original. É o gap crítico identificado na análise.

- [ ] Implementar `AslBoardResolver` em `df-model` ou `df-parser`
- [ ] Dado um `AslProgram` + `BoardProfile`, validar: (1) todos os pinos declarados em `hardware[n]` existem no `gpio` map do board; (2) o `io_mode` é compatível com o `type` do pino (ex: `output` num pino `analog` é erro); (3) o `target_role` é compatível com os `roles` declarados no GPIO map
- [ ] Produzir `Vec<DiagnosticMessage>` com localização (secção, campo, linha) para cada violação
- [ ] Golden tests: `basic-blink-logic.toon` + `arduino-uno-r3.toon` deve resolver sem erros; programa com pino inexistente deve produzir erro específico

### 0.4 — Bridge Tauri

- [ ] Configurar projeto [Tauri v2](https://v2.tauri.app/start/project-structure/) com frontend Svelte
- [ ] Implementar crate `df-bridge` com todos os handlers de comandos e listeners de eventos definidos em §4
- [ ] Tipagem TypeScript gerada automaticamente a partir dos tipos Rust (via tauri-specta ou tipos manuais espelhados)
- [ ] Teste de integração: frontend envia comando, Core responde com evento — ciclo completo validado

### 0.5 — UI Shell base

- [ ] Instalar e configurar [shadcn-svelte](https://www.shadcn-svelte.com/docs/installation) com `dendriForge-A11Y-Slate.css` como base de tokens
- [ ] Topbar global persistente com navegação entre os quatro ambientes: Simulação, GRAFCET, Ladder, HMI
- [ ] Layout docking: sidebars retráteis, painel central, suporte a resize
- [ ] Theme provider e propagação dos CSS custom properties do design system

---

## Fase 1 — Parser (Código → ASL)

> **Objectivo:** Processar código textual do utilizador, gerar AST e serializar em formato TOON canónico.

### 1.1 — Infraestrutura do Parser

- [ ] Definir interface abstrata `trait LanguageParser` em `df-parser`
- [ ] Cada linguagem implementa: `parse(source: &str) -> Result<AslProgram, ParseError>`
- [ ] Sistema de diagnósticos: erros e warnings com linha, coluna e mensagem
- [ ] Nenhum parser "meio pronto" é exposto via bridge até passar todos os golden tests

### 1.2 — Parser: Structured Text (ST)

- [ ] Escrever gramática PEG para ST com [pest](https://github.com/pest-parser/pest) e pest_derive
- [ ] Cobertura da especificação [IEC 61131-3 Structured Text](https://en.wikipedia.org/wiki/Structured_text): `IF/THEN/ELSE`, `CASE`, `FOR`, `WHILE`, `REPEAT`, `FUNCTION_BLOCK`, tipos primitivos (`BOOL`, `INT`, `REAL`, `TIME`), funções e métodos
- [ ] Mapeamento AST ST → `AslProgram` TOON: hardware interface, data, setup, loop, functions
- [ ] Golden tests: 5 programas ST de referência com output `.toon` esperado

### 1.3 — Parser: MicroPython

- [ ] Gramática PEG para o subconjunto MicroPython relevante para microcontroladores
- [ ] Mapeamento de `machine.Pin`, `time.sleep`, `utime`, `I2C`, `SPI`, `UART` para roles ASL
- [ ] Leverage do módulo `ast` do Python como referência para a gramática — a gramática é a do CPython, adaptada ao subconjunto MicroPython
- [ ] Golden tests: 5 programas MicroPython de referência com output `.toon` esperado

### 1.4 — Quality Gate Fase 1

- [ ] Round-trip completo: ST source → `AslProgram` → `.toon` → `AslProgram` idêntico
- [ ] Round-trip completo: MicroPython source → `AslProgram` → `.toon` → `AslProgram` idêntico
- [ ] Nenhum pânico em Rust para inputs malformados — apenas `Result::Err` com diagnóstico
- [ ] Testes de stress: ficheiros grandes, código inválido, caracteres especiais

---

## Fase 2 — Generator (ASL → Código)

> **Objectivo:** Ler o formato intermédio TOON e exportar código nativo compilável para as targets físicas.

### 2.1 — Infraestrutura do Generator

- [ ] Interface abstrata `trait LanguageGenerator` em `df-codegen`
- [ ] Cada generator implementa: `generate(program: &AslProgram, board: &BoardProfile) -> Result<String, CodegenError>`
- [ ] O generator recebe `boardFamilySkillId` e `defaultLanguageSkills` do board profile para seleccionar imports e padrões correctos para cada família
- [ ] Validação prévia obrigatória: o generator recusa input com `AslProgram` inválido ou que não passou o resolver board↔ASL

### 2.2 — Generator: Structured Text (ST)

- [ ] Geração de código ST válido para IEC 61131-3 a partir de `AslProgram`
- [ ] Mapeamento de `target_role` dos pinos para declarações de variáveis ST (`%IX`, `%QX`, `%MW`)
- [ ] Golden tests: `.toon` de entrada → `.st` de saída esperado
- [ ] Validação cruzada: o output ST deve ser re-parseável pelo Parser ST da Fase 1

### 2.3 — Generator: MicroPython

- [ ] Geração de código MicroPython para ESP32, Pico e RP2040
- [ ] Mapeamento de `boardFamilySkillId` para imports correctos (`machine`, `network`, `uasyncio`)
- [ ] Golden tests: `.toon` → `.py` esperado

### 2.4 — Quality Gate Fase 2

- [ ] Round-trip de ida e volta controlado: source → ASL → source_gerado — diferenças apenas de formatação, nunca de semântica
- [ ] Nenhum código gerado que não compile na target respectiva (validação offline com compilador)
- [ ] Coverage mínimo 90% nos crates `df-parser` e `df-codegen`

---

## Fase 3 — UI Barebones e Shell Global

> **Objectivo:** Interface limpa, compacta e orientada a dados, com topbar global e todos os painéis básicos.

### 3.1 — Topbar global

- [ ] Quatro separadores de ambiente: **Simulação**, **GRAFCET**, **Ladder**, **HMI**
- [ ] Indicador de estado do runtime (`Idle` / `Running` / `Paused` / `Fault`) com cor do token `--df-status-*`
- [ ] Menu de ficheiro: Novo, Abrir, Guardar, Guardar Como
- [ ] Notificações inline: erros e warnings do Core exibidos sem bloquear UI

### 3.2 — Stores globais Svelte

Usando [Svelte 5 Runes](https://svelte.dev/docs/svelte/what-are-runes) com `$state` e `$derived`:

- [ ] `projectStore` — projeto aberto, dirty flag, path
- [ ] `runtimeStore` — estado da simulação, tick count, variáveis de pin
- [ ] `ioTableStore` — tabela global de I/O e tags
- [ ] `diagnosticStore` — lista de erros e warnings do Core
- [ ] `selectionStore` — elemento seleccionado no canvas activo
- [ ] Regra: nenhuma store duplica dados do Core; apenas espelha projecções recebidas via eventos

### 3.3 — Quality Gate Fase 3

- [ ] Navegação entre ambientes sem perda de estado
- [ ] Topbar responsiva e funcional em 1280px+
- [ ] Stores correctas: nenhuma mutation directa de estado do Core pela UI
- [ ] Testes de componentes UI com [Vitest](https://vitest.dev/) + @testing-library/svelte

---

## Fase 4 — Janela de Simulação

> **Objectivo:** Canvas interactivo onde o utilizador compõe um circuito e executa simulação comportamental.

### 4.1 — Canvas Konva.js

- [ ] Instalar e configurar [Konva.js](https://konvajs.org/docs/index.html) no contexto Svelte
- [ ] Renderização de boards como SVG no canvas, com posicionamento e drag-and-drop
- [ ] Renderização de componentes como SVG, com drag-and-drop e snap-to-grid
- [ ] IDs CSS de pinos (`data-pin`, `data-type`, `class="digital"`) lidos do SVG → posições de conexão
- [ ] Fios virtuais com algoritmo de roteamento ortogonal (ângulos de 90°)
- [ ] Hit testing correcto para selecção de boards, componentes e fios
- [ ] Zoom, pan e fit-to-view

### 4.2 — Sidebar Esquerda 1 — Biblioteca de Componentes

- [ ] Carregamento dos perfis de boards e componentes via `invoke('load_library')`
- [ ] Sistema de categorias: Maker, Industrial, Passives, Sensors, Actuators, Hydraulics, Pneumatics, HMI
- [ ] Pesquisa alfabética em tempo real por nome e tags
- [ ] Drag do painel para o canvas → `invoke('add_board' | 'add_component', ...)`
- [ ] Estado retrátil persistido na sessão

### 4.3 — Sidebar Esquerda 2 — Painel de Propriedades

- [ ] Modo **elemento seleccionado:** exibe e edita atributos TOON do nó (label, parâmetros, pino ID)
  - Formulário gerado dinamicamente a partir dos campos `params` do perfil do componente
  - Alteração envia `invoke('patch_node_props', ...)` → Core valida → evento `layout_snapshot` actualiza UI
- [ ] Modo **sem selecção:** exibe propriedades globais do Workplace (nome do projeto, clock de simulação, notas)

### 4.4 — Sidebar Direita — Editor de Código

- [ ] [CodeMirror 6](https://codemirror.net/) integrado com Svelte
- [ ] Dropdown de linguagem: ST, MicroPython, C++, Rust — alimentado por `boardProfile.compatibility.languages`
- [ ] Syntax highlighting: extensões CodeMirror para cada linguagem suportada
- [ ] Botão "Compilar para ASL": envia `invoke('parse_source', ...)` → resultado exibido inline
- [ ] Botão "Exportar código": `invoke('generate_code', ...)` → download do ficheiro gerado
- [ ] Indicadores de linha e coluna para diagnósticos do Core

### 4.5 — Runtime de Simulação (Core)

- [ ] Clock determinístico no crate `df-sim` usando [tokio](https://github.com/tokio-rs/tokio) com tick fixo configurável
- [ ] Ciclo de execução: `sample_inputs` → `execute_logic` → `commit_state` → `emit_events`
- [ ] Equações matemáticas dos perfis de componentes executadas pelo motor
- [ ] Continuidade eléctrica virtual calculada no Core, não na UI
- [ ] Máquina de estados estrita:
  - `PLAY`: Core inicia ciclo de varredura
  - `PAUSE`: Core congela clock, preserva variáveis
  - `STOP`: Core destrói instância, UI retorna ao estado zero
- [ ] Snapshots opcionais para inspecção e replay de bugs

### 4.6 — Sincronização SVG com Estado de Simulação

- [ ] Eventos `pin_state_changed` do Core → Konva actualiza `data-state` nos SVGs via CSS do design system
- [ ] LEDs respondem a `data-state="on/off"` com glow/filter (já definido em `dendriForge-A11Y-Slate.css`)
- [ ] Estado de TX/RX, power LEDs e reset button conforme CSS existente
- [ ] Animações CSS dos componentes dinâmicos sincronizadas com o tick do Core

### 4.7 — Quality Gate Fase 4

- [ ] Simular `basic-blink-logic.toon` com Arduino Uno — LED pisca no canvas com timing correcto
- [ ] Simular `dual-button-led-control.toon` — dois botões controlam LED
- [ ] `Play` → `Pause` → `Resume` preserva estado das variáveis
- [ ] `Stop` → `Play` reinicia do zero sem resíduos de estado
- [ ] Nenhuma lógica de simulação duplicada na UI

---

## Fase 5 — Editor GRAFCET

> **Objectivo:** Modelação visual de lógicas sequenciais, convertida em TOON canónico para execução no Core.

### 5.1 — Canvas Svelte Flow

- [ ] Instalar e configurar [Svelte Flow](https://svelteflow.dev/) com nós customizados
- [ ] Nó **Etapa (Step):** quadrado, número de etapa, acções associadas, estado activo/inactivo visual
- [ ] Nó **Etapa Inicial:** dupla borda conforme norma IEC 61131-3
- [ ] Elemento **Transição:** linha horizontal com condição lógica editável
- [ ] **Divergência/Convergência AND e OR:** suportadas como nós especializados
- [ ] Edges direccionados ligando Steps → Transições → Steps seguintes

### 5.2 — Tabela de I/O integrada

- [ ] Componente [TanStack Table v8](https://tanstack.com/table/latest/docs/introduction) retrátil no topo do editor
- [ ] Colunas: Endereço, Tipo (DI/DO/AI/AO), Label, Tag, Valor actual
- [ ] Variáveis declaradas aqui alimentam as condições de transição e acções das etapas
- [ ] Partilha a `ioTableStore` com os outros editores — tabela global única

### 5.3 — Conversão GRAFCET → TOON

- [ ] O grafo Svelte Flow é convertido em objectos TOON de steps e transições via `invoke('update_grafcet_graph', ...)`
- [ ] O Core valida o GRAFCET: etapa inicial obrigatória, transições completas, ausência de deadlocks óbvios
- [ ] O resultado é integrado no `AslProgram` canónico para simulação

### 5.4 — Quality Gate Fase 5

- [ ] Criar GRAFCET simples (3 etapas, 2 transições) e simular com saídas visíveis no canvas
- [ ] Simular `plc-sequence-ap-bp-am-bm.toon` via GRAFCET — sequência pneumática A+/B+/A-/B-
- [ ] Tabela de I/O partilhada correctamente com o módulo de Simulação
- [ ] Exportar GRAFCET para TOON válido que passe no validator

---

## Fase 6 — Editor Ladder

> **Objectivo:** Diagrama de contactos IEC 61131-3 com cálculo de continuidade em tempo real.

### 6.1 — Estrutura de dados

- [ ] Array de Rungs no Core: cada rung é um objecto independente com a sua lista de elementos
- [ ] Tipos de elementos: `contact-no`, `contact-nc`, `coil`, `coil-set`, `coil-reset`, `timer-ton`, `timer-tof`, `timer-tp`, `counter-ctu`, `counter-ctd`
- [ ] Representação TOON de cada rung e elemento — contrato definido antes de qualquer UI

### 6.2 — Canvas Svelte Flow adaptado

- [ ] Cada Rung é um nó horizontal em Svelte Flow com comportamento de grelha
- [ ] Dentro do Rung, elementos Ladder renderizados como SVGs reativos com estado visual
- [ ] Drag-and-drop de elementos da sidebar esquerda para rungs
- [ ] Conexão automática de elementos no mesmo rung (esquerda a direita)
- [ ] Branches paralelas dentro do rung (divergências AND)

### 6.3 — Sub-topbar do editor Ladder

- [ ] Status do PLC: "Simulando" / "Parado" com cores do design system
- [ ] Botões de controlo de fluxo (Play/Pause/Stop) — despacham para a máquina de estados do Core
- [ ] Botão "Adicionar Novo Rung" — `invoke('add_ladder_rung', ...)`
- [ ] Botão "Eliminar Rung" para rungs seleccionados

### 6.4 — Sidebar Esquerda — Paleta IEC 61131-3

- [ ] Contactos: NA, NF
- [ ] Bobinas: Normal, Set (S), Reset (R), Rising Edge, Falling Edge
- [ ] Temporizadores: TON, TOF, TP (com parâmetros PT e ET)
- [ ] Contadores: CTU, CTD (com parâmetros PV e CV)
- [ ] Instruções avançadas: MOVE, ADD, SUB, MUL, DIV, comparadores (EQ, NE, GT, LT)

### 6.5 — Cálculo de Continuidade

- [ ] O array de Rungs actualiza o TOON em tempo real via `invoke('update_rung_element', ...)`
- [ ] O Core calcula a continuidade eléctrica virtual a cada tick: energizado / não energizado
- [ ] Resultado propagado via `pin_state_changed` → elementos Ladder actualizam cor (energizado = verde)
- [ ] Mapeamento heurístico linear Ladder → TOON canónico

### 6.6 — Quality Gate Fase 6

- [ ] Simular `plc-elevator-advanced.toon` via editor Ladder — lógica de elevador completa
- [ ] Cálculo de continuidade correcto para branches paralelas
- [ ] TON e CTU funcionam com parâmetros editáveis
- [ ] Tabela de I/O partilhada correctamente

---

## Fase 7 — Editor HMI, SCADA e IoT

> **Objectivo:** Ambiente de design de ecrãs operacionais com arquitectura Data-Driven e binding de tags.

### 7.1 — Canvas com grid magnético

- [ ] [Konva.js](https://konvajs.org/docs/index.html) com snap-to-grid magnético
- [ ] Undo/Redo para operações de design de ecrã
- [ ] Layers separados: fundo, componentes estáticos, componentes dinâmicos

### 7.2 — Biblioteca de Widgets

**HMI/SCADA:**

- [ ] Motor (running/stopped/fault states)
- [ ] Tanque (nível como barra de preenchimento SVG animada)
- [ ] Válvula (aberta/fechada, actuação parcial)
- [ ] Indicador numérico (baseado em `core/screens/ind-hmi-numeric-indicator.toon`)
- [ ] Gráfico de tendências (baseado em `core/screens/ind-hmi-trend-chart.toon`)
- [ ] Botão industrial (NO, NC, pulsado)
- [ ] Luz piloto (verde/vermelho/amarelo/azul, com estados de `dendriForge-A11Y-Slate.css`)

**IoT:**

- [ ] Dashboard card com valor e unidade
- [ ] Gauge circular (medidor de 270°)
- [ ] Interruptor toggle
- [ ] Gráfico de linha temporal

> **Nota:** `core/screens/` tem actualmente apenas 2 ficheiros `ind-`. A expansão desta pasta com
> variantes `maker-` deve ocorrer em paralelo com o desenvolvimento desta fase.

### 7.3 — Importador de SVGs customizados

- [ ] Área de upload com drag-and-drop
- [ ] Análise dos IDs, classes e atributos `data-*` do SVG importado
- [ ] Interface de mapeamento: seleccionar elemento SVG → definir propriedade animada (`fill`, `opacity`, `transform`, `data-state`)
- [ ] Persistência do mapeamento no `assets/` do projeto

### 7.4 — Sidebar Direita — Data Binding

- [ ] Lista de variáveis da `ioTableStore` global
- [ ] Quando elemento HMI seleccionado: painel exibe variáveis disponíveis para binding
- [ ] Vinculação: `invoke('bind_tag', { elementId, tagName, property })`
- [ ] Preview em tempo real com dados simulados do Core
- [ ] Suporte a binding de: valor numérico, estado booleano, cor condicional, nível de preenchimento

### 7.5 — Quality Gate Fase 7

- [ ] Ecrã HMI com tanque + indicador numérico vinculado a variável do programa
- [ ] Simulação activa actualiza o ecrã em tempo real sem intervenção manual
- [ ] SVG customizado importado com binding funcional
- [ ] Layout guardado e recarregado correctamente

---

## Fase 8 — Persistência e Segurança

> **Objectivo:** Projetos guardados, seguros, reproduzíveis e sem dependência de cloud.

### 8.1 — Formato de projeto .dforge

- [ ] Implementar em `df-project`: save/load do formato ZIP descrito em §5
- [ ] Autosave transacional: escreve em `.dforge.tmp`, só renomeia para `.dforge` se commit bem-sucedido
- [ ] Detecção de dirty state: aviso ao utilizador antes de fechar com alterações não guardadas
- [ ] Migrações de versão: projetos antigos abertos com migração automática não destrutiva

### 8.2 — Segurança de assets

- [ ] Import de SVG com validação: sem `<script>`, sem `javascript:` URLs, sem event handlers inline
- [ ] SVG sanitizado antes de ser persistido no projeto
- [ ] Import de `.toon` externo valida contra spec antes de ser aceite
- [ ] Isolamento: ficheiros do projeto não executam código arbitrário — apenas o Core os interpreta

### 8.3 — Quality Gate Fase 8

- [ ] Save → Close → Reopen: estado idêntico ao guardado
- [ ] Autosave: falha de escrita não corrompe ficheiro existente
- [ ] SVG malicioso rejeitado com mensagem clara ao utilizador

---

## Fase 9 — Testes, Hardening e Documentação

> **Objectivo:** Cobertura completa, profiling, ausência de regressões e documentação mínima.

### 9.1 — Pirâmide de testes

| Nível | Ferramenta | Âmbito |
|---|---|---|
| Unit Rust | `cargo test` | Parser, validator, runtime, codegen, project |
| Golden tests | ficheiros `.toon` de referência em `tests/golden/` | Round-trip e output canónico |
| Snapshot tests | `insta` crate | Estruturas ASL canónicas |
| Integration tests | `cargo test` em `df-bridge` | Core ↔ Tauri bridge |
| UI tests | [Vitest](https://vitest.dev/) + @testing-library/svelte | Fluxos críticos de UI |
| Stress tests | scripts dedicados | Projetos grandes, muitos componentes, edge cases |

### 9.2 — Cobertura mínima

- `df-parser`: 95%
- `df-sim`: 90%
- `df-codegen`: 90%
- `df-project`: 90%
- `df-model`: 100% (tipos puros)
- UI: fluxos críticos cobertos, não cosmética

### 9.3 — Profiling e performance

- [ ] Medir tempo de parse para ficheiros `.toon` grandes (100+ componentes)
- [ ] Medir latência do tick de simulação e garantir determinismo
- [ ] Medir tempo de carga inicial do Core e carregamento da biblioteca
- [ ] Medir consumo de memória com projeto complexo activo

### 9.4 — Documentação mínima

- [ ] `README.md` do repositório com setup completo e como correr os testes
- [ ] Docstrings Rust (`///`) em todos os tipos públicos e funções da API bridge
- [ ] `ARCHITECTURE.md`: descrição das camadas, contratos e decisões de design
- [ ] Comentários em código não óbvio — sem comentários redundantes que apenas descrevem o que o código já diz

---

## Critérios de Pronto e Quality Gates

Uma feature está pronta quando cumpre **quatro condições simultâneas:**

1. **Comportamento correcto** — faz o que foi especificado
2. **Testes verdes** — unitários e de integração passam, sem bypass
3. **Documentação mínima** — tipos públicos e API bridge documentados
4. **Integração limpa** — não introduz lógica de domínio no Svelte, não duplica estado

Uma **fase** avança quando:

- Todos os quality gates da fase estão marcados ✅
- Nenhum teste de regressão das fases anteriores está a falhar
- O utilizador reviu e validou o comportamento esperado

---

## Escopo da Versão 0.1.0

A 0.1.0 é pequena, fechada e impecável. Não contém todas as features — contém as features certas, completamente implementadas.

### Incluído na 0.1.0

- [ ] M0 — Limpeza do repositório (6 artefactos)
- [ ] M0 — `core/components/rules/asl-component-profile.md` criado
- [ ] M0 — Resolver board↔ASL especificado em `docs/architecture/`
- [ ] Workspace Rust com todos os crates estruturados
- [ ] Loader e validator completo de TOON/ASL, boards, components e screens
- [ ] Parser para Structured Text (ST)
- [ ] Parser para MicroPython
- [ ] Generator para ST e MicroPython
- [ ] Shell UI com topbar e navegação entre ambientes
- [ ] Janela de Simulação: canvas, biblioteca, propriedades, editor de código, play/pause/stop
- [ ] Editor Ladder: rungs, elementos IEC 61131-3 básicos, continuidade em tempo real
- [ ] Editor GRAFCET: steps, transições, tabela de I/O
- [ ] Editor HMI: widgets base, binding de tags, SVG customizado
- [ ] Formato de projeto `.dforge`: save, load, autosave
- [ ] Testes com cobertura mínima definida
- [ ] Documentação de arquitectura

### Excluído da 0.1.0 (pós-release)

- [ ] Parser C++ e Rust/Embassy
- [ ] Assistente BYOK (Bring Your Own Key)
- [ ] MQTT/cloud e conectividade IoT real
- [ ] Colaboração multi-utilizador
- [ ] Plugins e extensibilidade externa
- [ ] SCADA avançado, PI, trends complexos
- [ ] Suporte mobile e web
- [ ] Themes adicionais (apenas Slate na 0.1.0)
- [ ] Exportação para PDF/HTML de documentação de projeto
- [ ] SVGs dos 43 boards em falta (podem ser adicionados incrementalmente após 0.1.0)
- [ ] Variantes `maker-` de `core/screens/`

---

## Expansão pós 0.1.0

| Versão | Foco |
|---|---|
| 0.2.0 | Parser C++ e Rust/Embassy. Suporte completo a `boardFamilySkillId`: `avr-family`, `embassy-*` |
| 0.3.0 | Assistente BYOK — integração de API de IA com markdown de treino customizado |
| 0.4.0 | Conectividade IoT — MQTT, WebSocket, broker cloud |
| 0.5.0 | SCADA avançado — historian, trends multi-canal, alarmes persistentes |
| 0.6.0 | Multi-janela, workspaces, plugins e API pública de extensibilidade |
| 1.0.0 | Estabilidade API, telemetria opt-in, distribuição oficial |

---

## Regras de Engenharia Imutáveis

1. **Nenhuma regra de negócio no Svelte.** Toda lógica de domínio vive no Core Rust.
2. **Nenhum estado duplicado entre UI e Core.** O Core é a única fonte de verdade.
3. **Nenhuma feature sem teste automatizado.** Uma feature sem teste não existe.
4. **Nenhuma expansão antes de fechar o escopo 0.1.0.** Foco até ao fim.
5. **Nenhum parser ou generator "meio pronto" exposto ao utilizador.**
6. **Nenhum asset SVG entra sem contrato de IDs/classes/data validado.**
7. **Nenhum código gerado que não compile na target respectiva.**
8. **Cada fase só avança com quality gate aprovado.**
9. **O formato `.toon` é imutável dentro da 0.1.0.** Mudanças de spec requerem nova versão e migração.
10. **Desktop first, sempre.** Compromissos de layout e performance favorecem sempre o desktop.
11. **O resolver board↔ASL é obrigatório antes de qualquer generator.** Código gerado para pinos inválidos não é gerado — é rejeitado com diagnóstico.
12. **`core/` é dados, não código.** Nenhum script, bytecode ou lógica de runtime pertence a `core/`.
