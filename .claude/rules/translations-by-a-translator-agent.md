# Translations come from a dedicated translator agent

❌ A coding agent never translates the strings it adds: it writes the English plus a good `@key` description, and a
separate translator agent does the rest, reading that language's `docs/i18n/<tag>/style.md` and searching its
`glossary.md` for the terms in play. **Why:** a coding agent once shipped 10 translations that passed every check
without opening a single style guide or glossary. When to fan out one agent per language:
`docs/guides/i18n-translation.md`.
