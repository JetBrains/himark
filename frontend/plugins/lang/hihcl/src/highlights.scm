; Hand-written minimal query — the hcl grammar repo ships no queries.
(comment) @comment
(numeric_lit) @number
(bool_lit) @constant
(null_lit) @constant
(string_lit) @string
(quoted_template) @string
(heredoc_template) @string
(block (identifier) @keyword)
(function_call (identifier) @function)
(attribute (identifier) @variable)
