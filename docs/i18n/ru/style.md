# Russian (ru) translation style guide

Working notes for translating Cmdr into Russian. Read `../README.md` for how this fits the translation process.

Russian is fully resourced in the pile: macOS Finder/AppKit, Microsoft terminology + full style guide, GNOME Nautilus +
Xfce Thunar. Lean on macOS Finder first.

## Digest

- Use concise, neutral Russian. Actions use infinitives; running text addresses users with lowercase «вы» or avoids
  direct address. Never infer the user's gender.
- Prefer Finder labels: «Скопировать», «Переименовать», «Отменить», «Свойства», «Быстрый просмотр», «Системные
  настройки». Names of controls are quoted exactly as displayed.
- Keep Cmdr, macOS, Finder, GitHub, protocol names and model identifiers recognizable. Ask Cmdr is «Спросить Cmdr»; AI
  is «ИИ»; API key is «API-ключ».
- Use Cyrillic for Russian words, mostly «е» rather than «ё», and sentence case. Primary quotes are «…», nested quotes
  „…“, ellipsis is …, apostrophe is ’ when needed. Never use ASCII quotes in UI prose.
- Every ICU count requires one/few/many/other. The one category includes 21, 31 and 101, not just 1; retain the numeric
  placeholder. Use =1 only when exactly one needs distinct wording.
- Keep raw paths and names in neutral slots. Never attach an unknown grammatical ending or refer back with a pronoun. No
  bracketed or slashed number/gender endings.
- Accessible names contain the visible label verbatim and in order, allowing only case differences.
- Full view is «подробный режим», brief view «краткий режим», pane «панель». Finder lacks two-pane vocabulary, so those
  rulings remain tentative until checked against orthodox file managers.
- See the [termbase](terms.json), [typography rules](mechanics.json), [translation decisions](decisions.md) and
  [review queue](review-queue.md) for current evidence and open questions.

## Voice and tone

Friendly, concise, active, never alarmist. Russian tech UI is somewhat more formal and impersonal than English by
default; keep Cmdr's warmth but don't force colloquialisms. Microsoft's Russian style guide explicitly favors a neutral,
respectful register. Error messages stay calm and actionable; avoid alarmist words.

## Formality

Russian UI generally **avoids a personal form of address in action labels**, using infinitives for commands and nouns
for categories. This is the dominant macOS + Microsoft convention:

- Use the **infinitive** for menu/button actions: "Скопировать" (copy), "Переместить" (move), "Переименовать" (rename),
  "Удалить" (delete) are infinitives used as commands, this is the standard, NOT the imperative "Скопируй". Apple and
  Microsoft both use the infinitive-as-command throughout.
- When running text must address the user, use the polite **вы** (lowercase in modern tech UI; uppercase "Вы" is older
  correspondence style). Microsoft's style guide prescribes lowercase "вы". Never the familiar **ты** in product UI.
- Prefer impersonal/passive constructions for system messages ("Файл удалён", "the file was deleted") where English uses
  active; this reads natural in Russian even though Cmdr's English prefers active voice. Don't over-apply: keep it
  concise.

## Decision points

### Script: Cyrillic only (no decision, but lock it)

Russian is Cyrillic, full stop. No Latin transliteration in UI. The only trap is mixing visually identical
Latin/Cyrillic letters (e.g. Latin "c"/"a"/"o" inside a Cyrillic word), keep all-Cyrillic in Russian words.
Recommendation: pure Cyrillic. Confidence: high. Not a real decision, just a correctness guard.

### "ё" vs "е"

Russian optionally writes **ё** but it's very often replaced by **е** in print and UI. Apple and Microsoft generally use
**е** (without dots) in UI except where ё disambiguates. Recommendation: follow the macOS Finder convention (mostly
**е**, ё only where needed for clarity); be consistent across the catalog. Confidence: high.

### Grammatical case agreement with counts and inserted values

The biggest Russian translation hazard. Nouns take different case/number forms after numbers (1 файл, 2 файла, 5
файлов), and any `{placeholder}` carrying a count or a noun phrase can land in the wrong case. This is handled by the
plural categories (see below), but ALSO affects non-count inserts: a `{path}` or `{name}` dropped into a sentence keeps
nominative form, so structure sentences so the insert sits in a position that reads correctly regardless of its
grammatical gender/number. Recommendation: write count messages with full one/few/many/other branches, and phrase
sentences with raw inserts so the insert is in an isolated nominative slot (e.g. "Файл: {name}" not "Перемещение
{name}"). Confidence: high. This is the #1 source of clumsy Russian translations.

### Gender and inclusive language

Russian is heavily gendered, including in **past-tense verbs** that agree with the subject's gender ("удалил" masc. vs
"удалила" fem.). If a message ever has the USER as the past-tense subject ("you deleted"), it would force a gender.
Avoid entirely: use impersonal/passive ("Файл удалён", neuter, agrees with "файл", not the user) or the infinitive.
There is NO accepted gender-neutral morphology in Russian product UI; Apple/Microsoft/Google/Spotify/Netflix all avoid
the problem structurally rather than inventing neutral forms. Recommendation: phrase around user-gendered past tense;
never invent neutral endings. Confidence: high.

## Terminology and glossary

Defer the full glossary; triangulate macOS Finder (highest) + Microsoft terminology + Nautilus/Thunar.

| English term | Russian     | Notes                 |
| ------------ | ----------- | --------------------- |
| file         | файл        |                       |
| folder       | папка       |                       |
| trash        | Корзина     | Finder term           |
| copy         | Скопировать | infinitive-as-command |
| pane         | панель      | confirm vs Finder     |
| tab          | вкладка     |                       |

## Brand and do-not-translate

Keep verbatim: Cmdr, macOS, GitHub, SMB, MTP, Tauri, Rust, Svelte. Apple localizes Quick Look as **Быстрый просмотр**,
verified in live Finder's LocalizableMerged.json (N169.17). Enforced by `desktop-i18n-dont-translate`.

## Plurals

CLDR categories for `ru`: `one`, `few`, `many`, `other`. All four are required and grammatically real:

- `one`: 1, 21, 31… (файл)
- `few`: 2-4, 22-24… (файла)
- `many`: 0, 5-20, 25-30… (файлов)
- `other`: fractionals (файла) Every count message MUST write all four branches with the correctly cased noun form; this
  is non-optional and the most common Russian plural bug is omitting `many`.

## Notes and decisions

- Quotation marks: Russian uses guillemets «...» for primary quotes and „..." (low-9/high-9) for nested. Use «...», not
  English "...".
- Decimal comma, space thousands separator (non-breaking). Let `Intl` format.
- No serial/Oxford comma in Russian (English Cmdr style uses it; Russian punctuation rules differ, follow Russian norms,
  not the English style guide, for in-language punctuation).

## Decisions to confirm with David

- None blocking. The impersonal/passive-by-default register (vs Cmdr's English active-voice preference) is a deliberate
  call worth confirming once with a native reviewer, but it matches every major.

## Termbase

The [Russian termbase](terms.json) records current term choices and evidence. General file-manager labels are sourced
from the live Russian Finder resources extracted into the reference pile. Technical and two-pane concepts absent from
that extraction are marked tentative rather than presented as externally verified.

See the [distilled decisions](decisions.md), [typography rules](mechanics.json) and
[native review queue](review-queue.md). Native review improves quality opportunistically and does not block shipping
under the project translation guide.

&nbsp;

---

_02.10.2026_
