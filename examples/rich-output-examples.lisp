;;;; rich-output-examples.lisp
;;;;
;;;; Examples of rich output features in Zed Common Lisp REPL
;;;;
;;;; Zed supports these MIME types:
;;;; - text/plain (plain text)
;;;; - text/markdown (formatted markdown)
;;;; - application/json (JSON viewer - preview builds only, after Jan 30, 2026)
;;;; - image/png (PNG images, base64-encoded)
;;;; - image/jpeg (JPEG images, base64-encoded)
;;;; - application/vnd.dataresource+json (data tables)
;;;;
;;;; All display functions are in the zed-cl package.
;;;; Use zed-cl:display-markdown, zed-cl:display-table, etc.

;; Simple markdown output
(zed-cl:display-markdown "# Hello World")

(zed-cl:display-markdown "# Hello from Common Lisp!

This is **bold** and this is *italic*.

## Features
- Markdown rendering
- Code blocks
- Tables")

;; Table display (rendered as markdown table)
(zed-cl:display-table '(("Alice" 30 "Engineer")
                        ("Bob" 25 "Designer")
                        ("Charlie" 35 "Manager"))
                      :headers '("Name" "Age" "Role"))

;; display-describe shows object metadata using DESCRIBE
;; Good for: packages, classes, symbols, functions
(zed-cl:display-describe *package*)

;; For a class instance, display-describe shows type info and slots
(defclass person ()
  ((name :initarg :name :accessor person-name)
   (age :initarg :age :accessor person-age)
   (occupation :initarg :occupation :accessor person-occupation)))

(zed-cl:display-describe (make-instance 'person
                                        :name "Bob"
                                        :age 25
                                        :occupation "Designer"))

;; NOTE: For hash tables, use display-hash-table (defined below)
;; instead of display-describe to see the actual key-value pairs

;; List display (unordered)
(zed-cl:display-list '("First item"
                       "Second item"
                       "Third item"))

;; List display (ordered)
(zed-cl:display-list '("Step 1: Initialize"
                       "Step 2: Process"
                       "Step 3: Finalize")
                     :ordered t)

;; Markdown with code blocks
(zed-cl:display-markdown "## Example Code

```lisp
(defun factorial (n)
  (if (<= n 1)
      1
      (* n (factorial (- n 1)))))
```

The function computes **factorial** recursively.")

;; Multiple outputs in one evaluation
(progn
  (zed-cl:display-markdown "### Multiple Outputs Example")
  (zed-cl:display-table '((1 2 3) (4 5 6) (7 8 9))
                        :headers '("A" "B" "C"))
  (zed-cl:display-markdown "This evaluation produced **multiple** rich outputs!")
  "Final return value: 42")

;; Practical example: Display function documentation
(defun show-function-info (function-name)
  "Display detailed information about a function with markdown formatting"
  (let ((doc (documentation function-name 'function))
        (arglist (when (fboundp function-name)
                  (sb-introspect:function-lambda-list function-name))))
    (zed-cl:display-markdown
     (format nil "## ~A

**Arguments:** `~A`

~A"
             function-name
             (or arglist "unknown")
             (if doc
                 (format nil "**Documentation:**

~A" doc)
                 "*No documentation available*")))
    function-name))

;; Try it out
(show-function-info 'mapcar)

;; Practical example: Display hash table contents
(defun display-hash-table (ht &key (title "Hash Table Contents"))
  "Display a hash table as a markdown table"
  (let ((rows nil))
    (maphash (lambda (k v)
              (push (list (prin1-to-string k)
                         (prin1-to-string v))
                    rows))
            ht)
    (zed-cl:display-table (nreverse rows)
                          :headers '("Key" "Value"))
    (zed-cl:display-markdown (format nil "**~A** (count: ~D)" title (hash-table-count ht)))))

;; Try it out
(let ((ht (make-hash-table :test 'equal)))
  (setf (gethash "name" ht) "Alice")
  (setf (gethash "age" ht) 30)
  (setf (gethash "role" ht) "Engineer")
  (display-hash-table ht :title "Employee Data"))

;; You can mix regular output with rich output
(progn
  (format t "Regular console output~%")
  (zed-cl:display-markdown "**Rich markdown output**")
  (+ 1 2 3))  ; Returns 6

;; JSON display example (shown as formatted code block in Zed stable)
(zed-cl:display-json "{\"name\": \"Alice\", \"age\": 30, \"skills\": [\"Lisp\", \"Python\", \"Rust\"]}")

;; Example showing optional :metadata parameter syntax
;; Note: metadata is rarely needed - it's for advanced custom rendering
;; Most display functions work fine without it
(zed-cl:display-table '(("x" 1) ("y" 2) ("z" 3))
                      :headers '("Variable" "Value")
                      :metadata nil)  ; Optional parameter shown for demonstration

;; Image display example (PNG)
;; This is a 10x10 red square PNG (base64-encoded)
(zed-cl:display-image
 "iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAYAAACNMs+9AAAAFUlEQVR42mP8z8BQz0AEYBxVSF+FABJADveWkH6oAAAAAElFTkSuQmCC"
 "image/png")

;; For your own images:
;; 1. Install cl-base64: (ql:quickload :cl-base64)
;; 2. Read and encode your image file:
;;    (with-open-file (in "path/to/image.png" :element-type '(unsigned-byte 8))
;;      (let* ((bytes (make-array (file-length in) :element-type '(unsigned-byte 8)))
;;             (_ (read-sequence bytes in))
;;             (base64 (cl-base64:usb8-array-to-base64-string bytes)))
;;        (zed-cl:display-image base64 "image/png")))
