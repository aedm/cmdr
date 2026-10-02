# Russian translation decisions

## Finder command labels: `menu.edit.copy`, `menu.file.rename`, `menu.file.getInfo`

Use «Скопировать», «Переименовать» and «Свойства»: live Russian Finder MenuBar and Localizable resources provide these
exact labels. The style guide's earlier «Копировать» seed is superseded.

## Localized Apple feature: `menu.file.quickLook`

Use «Быстрый просмотр», not the English feature name: live Russian Finder LocalizableMerged.json N169.17 explicitly
localizes Quick Look.

## Counts and uncontrolled names: `askCmdr.renameUndo.applied`, `settings.mediaIndex.reclaim.line`

Use all four Russian plural categories, retaining countText/# for 21 and 101 as well as 1. Restructure paths and names
into neutral slots rather than guessing gender or case.

## Product feature names: `askCmdr.title`, `settings.section.ai`

Use «Спросить Cmdr» and «ИИ» consistently. Cmdr and provider/model brands stay recognizable; API keys are «API-ключи».
External evidence for these newer technical labels remains tentative.
