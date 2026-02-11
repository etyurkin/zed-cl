;;;; display.lisp - Rich display utilities for Jupyter integration
;;;;
;;;; This file provides functions for creating rich, interactive outputs
;;;; in the Zed REPL and Jupyter notebooks.

(defpackage :zed-cl
  (:use :cl)
  (:export #:display
           #:display-markdown
           #:display-image
           #:display-json
           #:display-table
           #:display-describe
           #:display-list
           #:clear-display-outputs))

(in-package :zed-cl)

;;; Global variable to store display data for the current evaluation
(defvar *display-outputs* nil
  "List of display data to be sent with the next evaluation result")

;;; Display data structure
(defstruct display-item
  "Represents a single rich display output"
  (data (make-hash-table :test 'equal) :type hash-table)
  (metadata nil :type (or null hash-table)))

;;; Core display function
(defun display (data &key metadata)
  "Queue display data to be sent to Jupyter.
   DATA should be a hash table mapping MIME types to content strings.
   Example: (let ((ht (make-hash-table :test 'equal)))
              (setf (gethash \"text/markdown\" ht) \"# Hello\")
              (zed-cl:display ht))"
  (let ((item (make-display-item :data data :metadata metadata)))
    (push item *display-outputs*))
  nil)

;;; Markdown Display
(defun display-markdown (markdown &key metadata)
  "Display Markdown content.
   Example: (zed-cl:display-markdown \"# Hello\\n\\nThis is **bold**\")"
  (let ((data (make-hash-table :test 'equal)))
    (setf (gethash "text/markdown" data) markdown)
    (display data :metadata metadata)))

;;; Image Display (base64-encoded)
(defun display-image (image-data mime-type &key metadata)
  "Display an image. IMAGE-DATA should be base64-encoded string.
   MIME-TYPE should be \"image/png\", \"image/jpeg\", or \"image/svg+xml\".
   Example: (zed-cl:display-image base64-png-data \"image/png\")"
  (let ((data (make-hash-table :test 'equal)))
    (setf (gethash mime-type data) image-data)
    (display data :metadata metadata)))

;;; JSON Display
(defun display-json (json-string &key metadata)
  "Display JSON data as formatted markdown code block.
   Note: Zed stable (v0.221.5) doesn't support JSON viewer yet.
   JSON support was added in preview builds after Jan 30, 2026.
   Example: (zed-cl:display-json \"{\\\"name\\\": \\\"Alice\\\", \\\"age\\\": 30}\")"
  ;; Format as markdown code block since Zed stable doesn't support application/json yet
  (let ((formatted (format nil "```json~%~A~%```" json-string)))
    (display-markdown formatted :metadata metadata)))

;;; Table Display (as Markdown)
(defun display-table (rows &key headers metadata)
  "Display a table from a list of lists as markdown.
   ROWS is a list of row data (each row is a list of cells).
   HEADERS is an optional list of column headers.
   Example: (zed-cl:display-table '((\"Alice\" 30) (\"Bob\" 25))
                                  :headers '(\"Name\" \"Age\"))"
  (let ((markdown (with-output-to-string (s)
                    ;; Headers
                    (when headers
                      (format s "|")
                      (dolist (header headers)
                        (format s " ~A |" header))
                      (format s "~%|")
                      (dolist (header headers)
                        (declare (ignore header))
                        (format s " --- |"))
                      (format s "~%"))
                    ;; Rows
                    (dolist (row rows)
                      (format s "|")
                      (dolist (cell row)
                        (format s " ~A |" cell))
                      (format s "~%")))))
    (display-markdown markdown :metadata metadata)))

;;; Describe with markdown formatting
(defun display-describe (object &key metadata)
  "Describe an object with markdown code block formatting.
   Uses DESCRIBE to show object metadata and structure.
   Example: (zed-cl:display-describe *package*)"
  (let* ((desc-output (with-output-to-string (s)
                        (describe object s)))
         ;; Use princ instead of describe to avoid escape sequences
         ;; Or just use the output as-is since describe should give clean output
         (markdown (format nil "```~%~A```~%" desc-output)))
    (display-markdown markdown :metadata metadata)))

;;; List to markdown list
(defun display-list (items &key ordered metadata)
  "Display a list as markdown (ordered or unordered).
   Example: (zed-cl:display-list '(\"Item 1\" \"Item 2\" \"Item 3\") :ordered t)"
  (let ((markdown (with-output-to-string (s)
                    (loop for item in items
                          for i from 1
                          do (if ordered
                                 (format s "~D. ~A~%" i item)
                                 (format s "- ~A~%" item))))))
    (display-markdown markdown :metadata metadata)))

;;; Clear display outputs
(defun clear-display-outputs ()
  "Clear all queued display outputs"
  (setf *display-outputs* nil))

;;; Get and serialize display outputs for protocol
(defun get-display-outputs ()
  "Get all queued display outputs and clear the queue.
   Returns a list of plists suitable for serialization."
  (let ((outputs (reverse *display-outputs*)))
    (clear-display-outputs)
    (when outputs
      (loop for item in outputs
            collect (list :data (hash-table-to-alist (display-item-data item))
                         :metadata (when (display-item-metadata item)
                                    (hash-table-to-alist (display-item-metadata item))))))))

;;; Helper: Convert hash table to alist for serialization
(defun hash-table-to-alist (ht)
  "Convert a hash table to an alist"
  (when ht
    (let ((result nil))
      (maphash (lambda (k v)
                 (push (cons k v) result))
               ht)
      (nreverse result))))

