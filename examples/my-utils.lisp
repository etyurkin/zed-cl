;;;; my-utils.lisp
;;;;
;;;; Package MY-UTILS — define functions here, then call them from other files
;;;; (common-lisp-examples.lisp, using-shared-repl.lisp).
;;;;
;;;; Eval order: put the cursor on each form and run repl: run (Ctrl+Shift+Enter).
;;;; 1. defpackage
;;;; 2. in-package
;;;; 3. each defun
;;;; After that, other files in the same REPL can call my-utils:multiply-numbers.

(defpackage :my-utils
  (:use :cl)
  (:export #:add-numbers
           #:multiply-numbers
           #:format-greeting))

(in-package :my-utils)

(declaim (ftype (function (integer integer) integer) add-numbers))
(defun add-numbers (a b)
  "Add two numbers together"
  (+ a b))

(defun multiply-numbers (a b)
  "Multiply two numbers"
  (* a b))

(defun format-greeting (name)
  "Format a greeting message"
  (format nil "Hello, ~A!" name))

;; Not exported. Other packages reach it with my-utils::internal-helper.
(defun internal-helper (x)
  "Internal helper - not exported"
  (* x 2))
