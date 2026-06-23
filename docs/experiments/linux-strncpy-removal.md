# Linux Strncpy Removal Experiment

This experiment tested whether `ochna` can explain a recent Linux merge in a C-heavy corpus. The result was useful, but different from Kubernetes and Netty: the important facts were mostly deletions, so patch metadata was required and ochna was best used to validate the resulting current-tree symbol surface.

## Subject

- Repo: `torvalds/linux`
- Local submodule commit: `1a3746ccbb0a97bed3c06ccde6b880013b1dddc1`
- Subject: `Merge tag 'strncpy-removal-v7.2-rc1' of git://git.kernel.org/pub/scm/linux/kernel/git/kees/linux`
- Author date: `2026-06-19T21:56:45Z`
- Changed files: `19`

The merge removed the core kernel `strncpy()` API and per-architecture implementations after a long tree-wide migration to safer alternatives.

## Index State

The local Linux submodule already had an ochna index:

- `65221` files
- `1403920` nodes
- `2314240` edges

The submodule was shallow to one commit:

```bash
git rev-list --parents -n 1 HEAD
# 1a3746ccbb0a97bed3c06ccde6b880013b1dddc1
```

Because local parents were unavailable, the useful metadata source was the GitHub commit API:

```bash
gh api repos/torvalds/linux/commits/1a3746ccbb0a97bed3c06ccde6b880013b1dddc1
```

## What Changed

The merge removed:

- `strncpy` declaration from `include/linux/string.h`
- `strncpy` implementation and `EXPORT_SYMBOL(strncpy)` from `lib/string.c`
- `strncpy` FORTIFY wrapper from `include/linux/fortify-string.h`
- `fortify_test_strncpy` from `lib/tests/fortify_kunit.c`
- focused fortify tests:
  - `lib/test_fortify/write_overflow-strncpy-src.c`
  - `lib/test_fortify/write_overflow-strncpy.c`
- per-architecture implementations/declarations in alpha, m68k, powerpc, x86, and xtensa, including removal of `arch/alpha/lib/strncpy.S`

`Documentation/process/deprecated.rst` changed from warning about `strncpy()` to stating that it has been removed from the kernel. The replacement guidance now points to:

- `strscpy()` for NUL-terminated destinations
- `strscpy_pad()` for NUL-terminated and zero-padded destinations
- `memtostr()` / `memtostr_pad()` for fixed-width non-NUL source data
- `strtomem()` / `strtomem_pad()` for fixed-width non-NUL destinations
- `memcpy_and_pad()` for bounded runtime-size padded copies

## Ochna Workflow

Useful commands:

```bash
ochna status --json
ochna search strncpy --json
ochna node --file lib/string.c --symbols-only --json
ochna node --file include/linux/string.h --symbols-only --json
ochna node --file include/linux/fortify-string.h --symbols-only --json
ochna node --file lib/tests/fortify_kunit.c --symbols-only --json
```

What ochna confirmed:

- `lib/string.c::strncpy` is absent from the current index.
- `include/linux/string.h::strncpy` is absent from the current index.
- the FORTIFY `strncpy` wrapper is absent from `include/linux/fortify-string.h`.
- related but distinct APIs still exist and should not be mistaken for the removed core API:
  - `strncpy_from_user`
  - `strncpy_from_kernel_nofault`
  - `strncpy_from_user_nofault`
  - `tools/include/nolibc/string.h::strncpy`
  - helpers like `safe_strncpy`

## What Worked

Ochna was effective for current-tree symbol validation. After reading the merge metadata and patch, it quickly answered: "Is the core kernel `strncpy` symbol still indexed?" The answer was no.

It also helped avoid a false conclusion: searching `strncpy` still returns several symbols, but they are not the removed kernel API. The graph gives enough file and symbol context to separate `strncpy` from `strncpy_from_user` and tool-only `nolibc` helpers.

## What Did Not Work

Ochna alone could not explain the removal because the current index only models the current tree. The important story was that symbols and files disappeared. Without a prior index or patch metadata, absence is hard to distinguish from "never existed here."

Assembly-heavy changes were also outside ochna's current parser scope. The arch-specific removals are best understood from the patch/file list, not from current symbols.

## Plan Adjustment

For deletion-heavy C/Linux changes:

1. Use `gh api repos/torvalds/linux/commits/<sha>` for commit message, file list, and patch hunks.
2. Use ochna to validate the current symbol surface.
3. Treat same-prefix search hits as candidates that need classification, not proof the removed API still exists.
4. Consider a future `ochna diff --before ... --after ...` or two-index workflow for removed-symbol reports.

## Product Lesson

Linux showed that the next ochna improvement is not only better call resolution. For release archaeology, ochna also needs an optional historical/diff mode. Current-tree indexing is enough to validate what exists now; explaining deleted APIs requires either patch input or a previous index snapshot.
