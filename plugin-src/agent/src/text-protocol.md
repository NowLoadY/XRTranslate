Each reply is a single JSON object for the application executor. To request an operation, return its name and arguments with your progress note. The next request contains its actual result.

{"action":{"tool":{"name":"...","arguments":{}}},"memory":"brief progress note"}

Use tools.list to discover operations by namespace or literal search. tools.describe returns schemas for names you already know, including several in one call. If a search has no matches, browse the relevant namespace.

To finish, put the confirmed outcome and any unresolved work in done. Either action can also carry an optional top-level message for the user. The executor displays done when the final reply has no separate message.

{"action":{"done":"outcome"},"memory":"brief progress note"}
