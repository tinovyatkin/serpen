## type hint stripping task

We need to strip static type hints as they don't have any sense in the bundled source code.

Minimal requirements would be:

- stipping all type hints meta, including in string literal forms
- strip `typing` and similar modules import
- strip everything at `if typing.TYPE_CHECKING:` code branches
- remove `from __future__ import annotations`
- rewrite `typing.cast` function calls to just inner part

To approach this task we will start with research and creating and implementation strategy document / todo list.

### Basics / Specifications

Read carefully https://peps.python.org/pep-0484/ to have a deep and complete understand of Python static type hints
Use documents linked at https://peps.python.org/topic/typing/ when you need to dive deep on some particular topic.

### Previous work

Explore codebase of Python-powered type stripping utilities source code. You main target in this exploration is to understand complexity and collect testing examples, edge cases and non-trivial handling logic. Check source code at following directories:

- references/type-strip/strip-hints
- references/type-strip/python-minifier
- references/type-strip/TypeStripper

### Type handling with `rustpython-parser` and `rustpython-ast`

Explore following directories which contains implementation of tools based on `rustpython-parser` and `rustpython-ast` which also dealing with static type hints. Your main objective here is to collect practical code samples to identify and efficiently handle type hints with our parsing library. Check source code at following directories:

- references/type-strip/ruff
- references/type-strip/pyrefly

### Understanding runtime behavior

## Explore source code of RustPython to get even more insights of how dead branches (type hinting only logic) are identified at runtime, to efficiently handle `if TYPE_CHECKING` branches rewrite. Check source code at:

- references/type-strip/RustPython

## Document

Output comprehensive document at `docs/type-stripping-system-design.md` where outline requirements, challenges and glimpes in exiting implementation that you notices.
Write argumentation and your decision where type stripping logic should be implemented in our codebase - at AST rewriter, emitter, unparser or somewhere else.
Write down any specificity you learned about handling type hints with `rustpython-parser`.
Write a list of testing edge cases that we need to have.
Write detailed TODO list how you will approach the feature implementation.
