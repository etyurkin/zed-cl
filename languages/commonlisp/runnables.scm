; Define runnable code blocks for Common Lisp
; This tells Zed what to evaluate when cursor is in a form without selection

; Top-level list forms (defun, defvar, etc.)
(list_lit) @run

; Individual s-expressions
(sym_lit) @run
(str_lit) @run
(num_lit) @run
(kwd_lit) @run
