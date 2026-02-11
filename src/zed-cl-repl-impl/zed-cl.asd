;;;; zed-cl.asd - ASDF system definition for Zed-CL

(defsystem "zed-cl"
  :description "Common Lisp support for Zed editor"
  :version "0.1.0"
  :license "MIT"
  :depends-on ("zed-cl/config" "zed-cl/master-repl"))

(defsystem "zed-cl/config"
  :description "Shared configuration for Zed-CL"
  :version "0.1.0"
  :license "MIT"
  :depends-on ("cl-json")
  :components ((:file "config")))

(defsystem "zed-cl/compat"
  :description "Multi-implementation compatibility layer"
  :version "0.1.0"
  :license "MIT"
  :components ((:file "compat")))

(defsystem "zed-cl/master-repl"
  :description "Master REPL server shared across LSP and Jupyter kernel"
  :version "0.1.0"
  :license "MIT"
  :depends-on ("zed-cl/config" "zed-cl/compat")
  :components ((:file "compat")
               (:file "display")
               (:file "socket-server" :depends-on ("compat"))
               (:file "master-repl" :depends-on ("compat" "display" "socket-server"))))
