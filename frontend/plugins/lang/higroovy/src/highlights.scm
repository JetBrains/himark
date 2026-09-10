; Hand-written minimal query — the crate ships queries commented out
; and the grammar repo carries none.
(line_comment) @comment
(block_comment) @comment
(string_literal) @string
(decimal_integer_literal) @number
(decimal_floating_point_literal) @number
(hex_integer_literal) @number
(void_type) @type
["abstract" "as" "assert" "break" "case" "catch" "class" "continue"
 "def" "default" "do" "else" "enum" "extends" "final" "finally" "for"
 "if" "implements" "import" "in" "instanceof" "interface" "new"
 "package" "private" "protected" "public" "record" "return" "static"
 "switch" "synchronized" "throw" "throws" "try" "while" "yield"] @keyword
