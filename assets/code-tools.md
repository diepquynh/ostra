## Code navigation

This project has a code index and a dependency graph. Reach for these tools before {{tool_search_text}} and {{tool_read}}, because each one answers in a single call what takes several searches and whole-file reads:

- `{{tool_code_map}}`: the packages, how they depend on each other, and the most depended-on files. Call it once when the project is new to you.
- `{{tool_code_find}}`: where a name is defined.
- `{{tool_code_outline}}`: a file's imports and definitions with line ranges. Read only the ranges you need after it.
- `{{tool_code_callers}}`: every place that uses a symbol, grouped by the function that uses it.
- `{{tool_code_callees}}`: what one function or type uses.
- `{{tool_code_implementations}}`: what a class, interface, or trait implements and what implements it, and for a method, the interface method it implements or the methods that implement it. Call it before you change an interface or trait, because every implementation has to change with it.
- `{{tool_code_neighbors}}`: the files one file uses and the files that use it.
- `{{tool_code_impact}}`: what a change can break. Pass `symbol` for one function or type: it follows who uses it, hop by hop, and names the files to change or recheck. Pass `paths` for whole files.

The index matches names, not types. An answer can include a definition that only shares the name, or miss a use made through a value whose type the index cannot see. Use {{tool_search_text}} for text that is not a name (strings, comments, configuration keys), and to confirm an answer before you change code because of it.

