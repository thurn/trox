# Trox

Trox is a source-extraction localization system for Rust, TypeScript/TSX, and
static RON data. Developers write complete English messages beside their use
sites; Trox extracts them, expands locale-specific grammar into flat CSV rows,
and builds deterministic bundles for Rust and TypeScript runtimes. Messages
remain immutable, locale-independent `LocalizedString` values until an
explicit `Localizer` renders them at the presentation boundary, avoiding text
fragments, translator-authored programs, implicit locale state, and runtime
morphology.

Here is a Rust message with interpolation, exact-zero wording, and cardinal
plural selection:

```rust
use trox::prelude::*;

fn cards_remaining(card_count: u32) -> LocalizedString {
    txa(
        plural(card_count, [
            exact(0, "No cards remain."),
            one("{card_count} card remains."),
            other("{card_count} cards remain."),
        ]),
        tx_args![card_count],
        "Status text showing how many cards remain in the deck.",
    )
}

let value = cards_remaining(3);
let text = localizer.resolve(&value);
assert_eq!(text, "3 cards remain.");
```

## Capabilities and boundaries

Trox provides:

- Inline complete English messages.
- Named placeholders in Rust and TypeScript.
- Exact, cardinal, ordinal, and semantic selection.
- Static RON message extraction.
- Locale-specific row expansion.
- Non-destructive CSV synchronization.
- Deterministic JSON bundles.
- Compatible Rust and TypeScript runtimes.
- Canonical serialization of unresolved messages.
- Visible source fallback with diagnostics.

Trox does not provide:

- String-fragment composition.
- Translator-authored selector syntax.
- A general expression language.
- Automatic articles, case, or morphology.
- Arbitrary nested localized messages.
- Implicit global locale discovery.
- Runtime filesystem or network access.
- Currency, date, unit, or list formatting.
- Interpreted HTML or rich-text markup.

Design rules:

- Every selector leaf is a complete message.
- Translation files contain text, not programs.
- Terms are sparse, shallow, and explicit.
- Application code owns semantic inputs.
- Locale profiles own locale-private grammar.
- Deterministic tooling is authoritative.

## Repository

```text
.
├── crates/
│   ├── trox/          Rust API, bundles, wire values, resolver
│   └── trox-cli/      scanners, CSV workflow, bundle builder
├── packages/
│   └── trox/          `@trox/runtime` TypeScript package
├── conformance/       canonical cross-language fixtures
├── stress/
│   └── quest/         multilingual scenario corpus
├── Cargo.toml         Rust workspace
├── package.json       npm workspace
└── README.md          project documentation
```

Build and test:

```sh
cargo test --workspace
npm install
npm run typecheck
npm test
npm run build
```

Run the Quest workflow:

```sh
cargo run --release -p trox-cli --bin trox -- \
  --config stress/quest/trox.ron extract

cargo run --release -p trox-cli --bin trox -- \
  --config stress/quest/trox.ron check

cargo run --release -p trox-cli --bin trox -- \
  --config stress/quest/trox.ron bundle --allow-missing
```

Typical application workflow:

1. Add `tx` or `txa` calls to Rust or TypeScript.
2. Add flat `Tx(...)` records to configured RON data.
3. Declare runtime grammatical terms only when needed.
4. Configure inputs and outputs in `trox.ron`.
5. Run `trox check`.
6. Run `trox extract`.
7. Translate the generated CSV cells.
8. Run `trox check --deny warnings` in CI.
9. Run `trox bundle`.
10. Load a source bundle and a target bundle.
11. Resolve messages only at the presentation boundary.

## Rust syntax

Use the prelude in ordinary authoring modules:

```rust
use trox::prelude::*;
```

It exports:

- `LocalizedString`, `Localizer`, and `TermId`.
- `tx`, `txa`, `meaning`, and `tx_args!`.
- `plural`, `ordinal`, and `select`.
- `exact`, `zero`, `one`, `two`, `few`, `many`, and `other`.
- `when`, `otherwise`, and `TroxSelector`.
- `term`, `indefinite`, `counted`, and `opaque`.

### Static text

Use `tx` when there are no visible placeholders:

```rust
fn close_label() -> LocalizedString {
    tx(
        "Close deck browser",
        "Accessible label that closes the deck browser.",
    )
}
```

The final argument is required translator context. The result is not a host
`String`:

```rust
let value: LocalizedString = close_label();
let text: String = localizer.resolve(&value);
```

### Interpolation

Use `txa` for visible placeholders:

```rust
fn deck_label(deck_name: &str) -> LocalizedString {
    txa(
        "Deck: {deck_name}",
        tx_args![deck_name],
        "Label followed by the user-defined deck name.",
    )
}
```

Bind multiple values or map a different expression:

```rust
fn progress(owned_count: u32, total_count: u32) -> LocalizedString {
    txa(
        "Collection: {owned_count}/{total_count}",
        tx_args![
            owned_count,
            total_count => total_count,
        ],
        "Owned cards followed by total available cards.",
    )
}
```

`tx_args!` evaluates each expression once and converts it into an owned Trox
value. It borrows ordinary input variables by default.

Placeholder names match:

```text
[a-z][a-z0-9]*(?:_[a-z0-9]+)*
```

Valid:

```text
{name}
{card_name}
{player_2_name}
```

Rejected:

```text
{0}
{CardName}
{cardName}
{card.name}
{card-name}
{cárd_name}
```

Rules:

- Names use lowercase ASCII snake case.
- Names are at most 64 ASCII bytes.
- Every visible placeholder has one binding.
- Every binding appears in at least one leaf.
- Target rows may move, repeat, or omit declared placeholders.
- Target rows cannot invent placeholders.
- `{{` and `}}` render literal braces.

```rust
tx(
    "Write {{card_name}} to show a placeholder.",
    "Help text showing placeholder notation.",
)
```

### Meaning

Disambiguate identical text with a semantic meaning key:

```rust
fn open_button() -> LocalizedString {
    tx(
        meaning("open-action", "Open"),
        "Button label that opens the selected deck.",
    )
}

fn open_state() -> LocalizedString {
    tx(
        meaning("open-state", "Open"),
        "Status adjective for a room accepting players.",
    )
}
```

The meaning key changes message identity. It is not an arbitrary developer
message ID.

### Cardinal selection

Use `plural` for locale-dependent cardinal categories:

```rust
fn cards_remaining(card_count: u32) -> LocalizedString {
    txa(
        plural(card_count, [
            exact(0, "No cards remain."),
            one("{card_count} card remains."),
            other("{card_count} cards remain."),
        ]),
        tx_args![card_count],
        "Status text showing cards remaining in the deck.",
    )
}
```

Selection order:

1. Exact integer branch.
2. Target locale's CLDR category.
3. Matching category branch.
4. Required `other` branch.

Extraction adds target-only categories. A Russian CSV can receive `one`,
`few`, `many`, and `other` rows from an English source pattern.

The count need not be visible:

```rust
tx(
    plural(card_count, [
        one("A card remains."),
        other("Cards remain."),
    ]),
    "Status indicating whether one or multiple cards remain.",
)
```

### Ordinal selection

```rust
fn turn_position(turn_number: u32) -> LocalizedString {
    txa(
        ordinal(turn_number, [
            one("{turn_number}st turn"),
            two("{turn_number}nd turn"),
            few("{turn_number}rd turn"),
            other("{turn_number}th turn"),
        ]),
        tx_args![turn_number],
        "Label for a turn's ordinal position.",
    )
}
```

Plural and ordinal selectors accept nonnegative integers from zero through
`2^53 - 1`. Fractions, negative values, `NaN`, and infinities are invalid.

### Semantic selection

Define explicit stable keys:

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum BattleZone {
    Deck,
    Hand,
    Void,
}

impl TroxSelector for BattleZone {
    fn trox_key(&self) -> &'static str {
        match self {
            Self::Deck => "deck",
            Self::Hand => "hand",
            Self::Void => "void",
        }
    }
}
```

Select complete messages:

```rust
fn search_zone(zone: BattleZone) -> LocalizedString {
    tx(
        select(zone, [
            when(BattleZone::Deck, "Search your deck."),
            when(BattleZone::Hand, "Search your hand."),
            when(BattleZone::Void, "Search your void."),
            otherwise("Search that zone."),
        ]),
        "Rules text directing the player to search a zone.",
    )
}
```

Rules:

- `otherwise` is mandatory.
- Keys are stable serialization contracts.
- Keys are unique within the selector.
- Keys use one uniform string or boolean type.
- Generic `select` is not a numeric plural operation.
- Ordinary application branching stays in ordinary control flow.

Nested selectors are supported. Every final leaf must still contain the full
sentence, including articles, verbs, and punctuation.

### Terms and atoms

Map domain values to explicit term IDs:

```rust
impl CardSubtype {
    fn term_id(self) -> TermId {
        match self {
            Self::Warrior => TermId::new("card-subtype.warrior"),
            Self::Relic => TermId::new("card-subtype.relic"),
            Self::Event => TermId::new("card-subtype.event"),
        }
    }
}
```

Request an indefinite form:

```rust
txa(
    "Change {card_name} to become {subtype}.",
    tx_args![
        card_name => opaque(card_name),
        subtype => indefinite(subtype.term_id()),
    ],
    "Card transformation with an indefinite subtype.",
)
```

`indefinite(id)` is shorthand for:

```rust
term(id).form("indefinite")
```

It does not generate `a` versus `an` from spelling.

Request a counted form:

```rust
txa(
    "Create {creature_count} {creature_noun}.",
    tx_args![
        creature_count,
        creature_noun => counted(
            subtype.term_id(),
            creature_count,
        ),
    ],
    "Creation rule with a runtime-selected counted noun.",
)
```

`counted(id, number)` is shorthand for:

```rust
term(id).form("counted").number(number)
```

The count selects the term surface. The separate placeholder renders the
number.

Keep fixed vocabulary inline:

```rust
txa(
    plural(card_count, [
        one("Draw {card_count} card."),
        other("Draw {card_count} cards."),
    ]),
    tx_args![card_count],
    "Rules text instructing the player to draw cards.",
)
```

Create a term only when a runtime-selected concept needs a form, count, locale
facet, or deliberately centralized proper name.

`opaque(value)` inserts an atomic localized value. The nested value must be one
text leaf with no arguments or selectors. If it needs grammatical interaction,
model it as a term.

### Resolution and rejected forms

```rust
let text = localizer.resolve(&value);
let checked = localizer.resolve_checked(&value)?;
let json = value.to_canonical_json()?;
let decoded = localizer.localized_string_from_json(&json)?;
```

Decoding validates IDs, schemas, selectors, numeric ranges, terms, and limits.
Patterns, branches, arguments, and descriptions must stay inline. Dynamic
patterns, concatenation, missing descriptions, numeric `select`, `concat!`, and
arbitrary localized nesting are rejected.

## TypeScript syntax

```ts
import {
  Localizer,
  counted,
  exact,
  few,
  indefinite,
  one,
  opaque,
  ordinal,
  other,
  otherwise,
  plural,
  select,
  termId,
  two,
  tx,
  txa,
  when,
  type LocalizedString,
  type TermId,
} from "@trox/runtime";
```

Static and interpolated messages mirror Rust:

```ts
export function closeLabel(): LocalizedString {
  return tx(
    "Close deck browser",
    "Accessible label that closes the deck browser.",
  );
}

export function deckLabel(deck_name: string): LocalizedString {
  return txa(
    "Deck: {deck_name}",
    { deck_name },
    "Label followed by the user-defined deck name.",
  );
}
```

The argument object must be inline. Spreads and prebuilt objects are not
extractable.

Cardinal and ordinal selection:

```ts
export function cardsRemaining(
  card_count: number,
): LocalizedString {
  return txa(
    plural(card_count, [
      exact(0, "No cards remain."),
      one("{card_count} card remains."),
      other("{card_count} cards remain."),
    ]),
    { card_count },
    "Status text showing cards remaining in the deck.",
  );
}

export function turnPosition(
  turn_number: number,
): LocalizedString {
  return txa(
    ordinal(turn_number, [
      one("{turn_number}st turn"),
      two("{turn_number}nd turn"),
      few("{turn_number}rd turn"),
      other("{turn_number}th turn"),
    ]),
    { turn_number },
    "Label for a turn's ordinal position.",
  );
}
```

Semantic selection uses finite literal unions:

```ts
type BattleZone = "deck" | "hand" | "void";

export function searchZone(zone: BattleZone): LocalizedString {
  return tx(
    select(zone, [
      when("deck", "Search your deck."),
      when("hand", "Search your hand."),
      when("void", "Search your void."),
      otherwise("Search that zone."),
    ]),
    "Rules text directing the player to search a zone.",
  );
}
```

Map application values to explicit `TermId` values, then use `indefinite`,
`counted`, or `opaque` exactly as in Rust. Resolve and serialize explicitly:

```ts
const text: string = localizer.resolve(value);
const json: string = value.toCanonicalJSON();
const decoded = localizer.localizedStringFromJSON(json);
```

`LocalizedString` is not assignable to `string`. Host interpolation, prebuilt
or spread argument objects, and dynamic descriptions are rejected. Single and
double quotes and template literals without interpolation are accepted. The
scanner handles `.ts` and `.tsx` without module resolution.

## RON source extraction

RON is limited to flat static messages:

```ron
(
    internal_id: "close-deck-browser",
    label: Tx("Close deck browser"),
    accessibility_label: Tx(
        text: "Close deck browser",
        description: "Accessible label that closes the browser.",
    ),
    open_state: Tx(
        text: "Open",
        meaning: "open-state",
        description: "Status for a room accepting players.",
    ),
)
```

Rules:

- Only `Tx` records are extracted.
- Use the short `Tx("text")` form for plain static text.
- Use the named form when supplying `description` or `meaning`; `text` is
  required.
- `description` and `meaning` are optional named fields.
- Unwrapped strings are ignored.
- Placeholders and selectors are rejected.
- Argument maps, terms, and opaque values are rejected.
- `{{` and `}}` render visible braces.

Rejected by source extraction:

```ron
(
    ignored: "Close deck browser",
    placeholder: Tx(text: "Deck: {deck_name}"),
)
```

Configure description defaults for authored data:

```ron
ron_description_defaults: {
    "data/cards.ron": "Rules text for a game card.",
    "data/**/*.ron": "Player-authored game data label.",
},
```

An explicit description overrides a matching path default.

### Serde round-tripping

Application-owned Rust types may contain `LocalizedString` fields and derive
Serde normally:

```rust
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
struct Record {
    label: LocalizedString,
}

let record = Record {
    label: tx("Foo", "Example label."),
};
let encoded = ron::to_string(&record)?;
assert_eq!(encoded, r#"(label:Tx("Foo"))"#);

let decoded: Record = ron::from_str(&encoded)?;
assert_eq!(decoded, record);
```

This application-data representation uses the same `Tx` forms as source
extraction. It supports only flat static values. A value without a meaning
serializes as `Tx("Foo")`; a value with a meaning uses the named form
`Tx(text: "Open", meaning: "open-state")` so the meaning survives the round
trip. Dynamic patterns, arguments, and selectors return a serialization error.

Descriptions do not round-trip through `LocalizedString`. They are authoring
metadata and are not retained in the runtime value or message identity. For
example, `Tx(text: "Foo", description: "Example label.")` can be read, but
serializing the resulting value produces `Tx("Foo")`.

## Term catalog

`terms.ron` is one top-level map:

```ron
{
    "card-subtype.warrior": (
        description: "The Warrior card subtype.",
        value: "Warrior",
        forms: {
            "indefinite": "a Warrior",
            "counted": Number([
                One("Warrior"),
                Other("Warriors"),
            ]),
        },
    ),
}
```

Rules:

- A term has one stable ID and default value.
- Named forms are declared by project configuration.
- Numbered forms contain CLDR cardinal surfaces.
- Terms do not reference terms or messages.
- Source calls authorize exact form and number contracts.
- Fixed vocabulary stays in complete messages.

## Project configuration

Minimal complete `trox.ron`:

```ron
(
    source_locale: "en-US",
    terms: "terms.ron",
    source_bundle: "dist/locales/en-US.trox.json",
    source_report: "locales/en-US.csv",

    sources: [
        (
            language: Rust,
            include: ["src/**/*.rs"],
            exclude: [],
        ),
        (
            language: TypeScript,
            include: ["web/**/*.ts", "web/**/*.tsx"],
            exclude: [],
        ),
        (
            language: Ron,
            include: ["data/**/*.ron"],
            exclude: [],
        ),
    ],

    locales: {
        "es": (
            profile: "locales/es.ron",
            csv: "locales/es.csv",
            bundle: "dist/locales/es.trox.json",
        ),
    },

    max_expanded_rows_per_entry: 256,

    term_forms: {
        "counted": (
            description: "Term inflected for a cardinal count.",
            number: Required,
        ),
        "indefinite": (
            description: "Nonspecific singular noun phrase.",
            source_fallback: Default,
        ),
    },

    ron_description_defaults: {
        "data/**/*.ron": "Player-authored game data label.",
    },
)
```

The CLI discovers the nearest parent `trox.ron`. Select another complete
configuration with:

```sh
trox --config path/to/trox.ron check
```

## Locale profiles

Spanish example:

```ron
(
    locale: "es",
    direction: Ltr,
    isolation: Isolate,
    fallbacks: ["en-US"],

    facets: {
        "gender": (
            scope: Message,
            values: ["masculine", "feminine"],
        ),
    },

    term_facets: {
        "card-subtype.warrior": {
            "gender": "masculine",
        },
        "card-subtype.relic": {
            "gender": "feminine",
        },
    },
)
```

Profiles own:

- Locale identifier and fallback chain.
- Left-to-right or right-to-left direction.
- Placeholder isolation behavior.
- Locale-private facet IDs and values.
- Term classification under those facets.

Message-scoped facets expand containing messages into rows. Term-scoped facets
remain term context. Application code passes stable term IDs, not Spanish
gender or other locale-private metadata.

Trox does not guess grammar from spelling, person names, or machine
translation. Pass explicit semantic values when agreement requires them.

## Extraction and CSV

Commands:

```sh
# Validate without writes.
trox check
trox check --deny warnings
trox --json check

# Synchronize every locale or one locale.
trox extract
trox extract --locale ru

# Scaffold a locale.
trox locale init pl

# Build strict or development bundles.
trox bundle
trox bundle --locale es
trox bundle --allow-missing

# Remove reviewed obsolete rows.
trox prune --locale es
```

Extraction is transactional:

- It validates every requested output before replacement.
- Any failure leaves all requested outputs unchanged.
- Successful replacements are atomic.
- Repeated extraction without changes is byte-identical.
- Parallel scanning does not change output order.

Extraction preserves:

- Existing translations.
- Translator notes.
- Unknown workflow columns.
- Previous translations for changed rows.
- Removed rows as obsolete records.

Row state:

- New untranslated rows are missing.
- Translator-relevant source changes make rows stale.
- Removed source or expansion rows become obsolete.
- Nothing is deleted until explicit pruning.

Representative CSV:

```csv
entry_id,row_id,conditions,source,translation,status
tx1_ab,txr1_01,count.plural=one,{count} card,,missing
tx1_ab,txr1_02,count.plural=other,{count} cards,,missing
```

Managed IDs and condition columns are tool-owned. Translators edit surface text
and notes, not selectors.

Condition examples:

```text
card_count.exact=0
card_count.plural=few
turn_number.ordinal=other
zone.select=deck
subtype.gender=feminine
```

An exact translation cell containing `^` inherits the resolved translation
from the immediately preceding active row of the same entry:

```csv
conditions,translation
subtype.gender=masculine,Crear {count} Guerrero.
subtype.gender=feminine,^
```

Caret rules:

- `^` cannot cross an entry boundary.
- Blank cells remain missing.
- Consecutive carets resolve through prior active rows.
- Bundles materialize inherited text.
- Extraction protects meaning when adjacency changes.

Rows become stale when text, description, selector context, argument schema,
term-form request, or locale expansion changes. The old translation is kept in
`previous_translation`.

Run extraction before pruning. `prune` rejects a CSV that no longer matches the
current catalog, so a row that became active again is not deleted.

### Lint policy

```ron
lint: (
    default_warning: Warn,
    rules: {
        "trox.unchanged-translation": (
            level: Deny,
        ),
        "trox.human-row-expansion": (
            level: Allow,
            reason: "Reviewed translator cost in issue 123.",
        ),
    },
),
```

Rules:

- `Warn` reports a warning.
- `Deny` promotes a warning to an error.
- `Allow` requires a nonempty review reason.
- Reasons on `Warn` or `Deny` are invalid.
- Expansion above 32 rows requires reasoned suppression.
- The configured row limit remains a hard cap.

One-run overrides:

```sh
trox --lint trox.unchanged-translation=deny check

trox --allow \
  'trox.human-row-expansion=Reviewed translator cost in issue 123.' \
  check
```

JSON diagnostics include a stable rule ID, severity, path, source span when
known, explanation, and suggested correction. `trox.internal` means an
implementation failure, not an ordinary authoring error.

## Bundles and runtime

Every bundle operation emits:

- The configured source bundle.
- Every selected target bundle.

Deploy one source bundle and one active target bundle:

```text
source bundle ─┐
               ├─> Localizer ─> resolve(LocalizedString) ─> String
target bundle ─┘
```

Bundles are canonical JSON containing:

- Explicit format version.
- Pinned locale-data version.
- Source-catalog fingerprint.
- Canonical entry and row IDs.
- Target plural and ordinal behavior.
- Direction and isolation settings.
- Validated term forms and facet metadata.

The source bundle authorizes message identities, full signatures, visible
argument schemas, terms, and forms. Target bundles contain translated rows and
compatibility data.

Loading rejects unknown major versions, unsupported features, noncanonical
JSON, duplicate IDs, irreproducible IDs, invalid paths, and invalid signatures.

A catalog mismatch can preserve entry-level compatibility in normal mode.
Entries with matching IDs and full signatures remain usable; incompatible
entries use source fallback. Strict construction rejects any mismatch.

Runtime responsibilities:

- Resolve selectors with target locale rules.
- Format numeric placeholders as locale decimals.
- Resolve terms and message-scoped facets.
- Apply configured directional isolation.
- Reorder, repeat, or omit declared placeholders.
- Emit diagnostics with visible source fallback.

Application responsibilities:

- Load bundle bytes.
- Choose the active target locale.
- Construct and distribute the `Localizer`.
- Keep `LocalizedString` unresolved in non-presentation layers.
- Use checked resolution for tests and validation.

The runtime does not read files, fetch resources, discover locale state, mutate
globals, parse CSV, run extraction, or evaluate translator-authored code.

## Identity and compatibility

Entry IDs hash canonical JSON containing the identity version, meaning key,
pattern structure, source leaves, and branch order. Descriptions, locations,
runtime values, dynamic term IDs, and host predicates do not define identity.

Descriptions and argument contracts still affect source revision or signature,
so their changes can stale translations without changing the entry ID.

Stable IDs are at most 96 ASCII bytes and match:

```text
[a-z][a-z0-9]*(?:[.-][a-z0-9]+)*
```

Numeric branches use exact values in ascending order, followed by `zero`,
`one`, `two`, `few`, `many`, and `other`. Generic `select` keeps authored order
and puts `otherwise` last.

## Validation and conformance

`trox check` validates:

- Configuration and source globs.
- Rust, TypeScript, TSX, and RON syntax.
- Terms and named forms.
- Locale profiles and term classifications.
- Existing managed CSV state.
- Placeholders and selector branches.
- Target row expansion.
- Lint policy and reasons.

Runtime loading validates:

- Bundle and wire versions.
- Canonical encoding.
- Catalog fingerprints.
- Entry signatures and argument schemas.
- Selector values and paths.
- Term authorization.
- Expansion row IDs.

Shared fixtures cover static and dynamic identities, canonical wire bytes,
argument authorization, nested selectors, lookup, and fallback.

Before changing identity, bundle, or wire behavior, run:

```sh
cargo test --workspace
npm run typecheck
npm test

trox --config stress/quest/trox.ron extract
trox --config stress/quest/trox.ron check
trox --config stress/quest/trox.ron bundle --allow-missing
```

Confirm that a second extraction is byte-identical. The benchmark uses 100 MiB
of equal Rust, TypeScript, TSX, and RON source with 10,000 calls:

```sh
trox benchmark \
  --synthetic-mib 100 \
  --iterations 5
```

Gates are warm startup below 50 ms, throughput of at least 200 MiB/s, peak RSS
below twice the source size, and an optional 15 percent median regression cap.

## Terminology

- **Argument**: runtime value bound to a visible placeholder.
- **Atomic localized value**: text leaf with no arguments or selectors,
  inserted explicitly through `opaque`.
- **Bundle**: deterministic runtime JSON for source or target locale data.
- **Cardinal**: quantity category selected by `plural`.
- **Caret inheritance**: CSV `^` shorthand for the preceding active row.
- **Catalog fingerprint**: digest pairing compatible source and target bundles.
- **Condition**: read-only CSV label for one selector or facet choice.
- **Description**: translator context attached to a message or term.
- **Entry**: one extracted message family or term-form family.
- **Entry ID**: content-derived versioned message identifier.
- **Exact branch**: numeric branch checked before a CLDR category.
- **Facet**: locale-owned term metadata such as grammatical gender.
- **Fallback**: visible source text used when target data is unusable.
- **Form**: declared semantic term surface such as `indefinite`.
- **Identity descriptor**: canonical structure hashed into an entry ID.
- **Locale profile**: direction, isolation, fallback, and facet configuration.
- **LocalizedString**: immutable unresolved message value.
- **Localizer**: resolver built from one source and one target bundle.
- **Meaning key**: discriminator for identical text with different semantics.
- **NFC**: required Unicode Normalization Form C.
- **Opaque**: assertion that a localized value is grammatically atomic.
- **Ordinal**: position category selected by `ordinal`.
- **Pattern**: text leaf or finite selector tree of complete leaves.
- **Placeholder**: named insertion point such as `{card_name}`.
- **RON**: Rusty Object Notation used for checked static data.
- **Row**: one translation unit after locale-specific expansion.
- **Scalar**: visible text, finite number, or boolean argument.
- **Select**: finite semantic branch using a stable application value.
- **Selector**: plural, ordinal, or semantic pattern node.
- **Source catalog**: runtime authority for identities and argument contracts.
- **Source report**: source-locale CSV describing extracted rows.
- **Source revision**: translator context that may stale a row.
- **Source signature**: digest proving exact source compatibility.
- **Stale row**: active row whose source context changed.
- **Term**: stable lexical concept used for runtime grammar.
- **Translation row**: flat CSV record containing one editable pattern.
- **Wire value**: canonical serialized `LocalizedString`.
