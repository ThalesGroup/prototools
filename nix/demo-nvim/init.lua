-- Neovim config for the demos: teleprompt's (spec 0395), and protolens's
-- `v`, through the demos' VIMINIT (spec 0420 S8).
-- Shared with the dev-shell's _hook_nvim highlighting (nix/shells.nix).
-- SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
-- SPDX-License-Identifier: MIT

-- ── 1. Aesthetics ────────────────────────────────────────────────────────────

vim.cmd("syntax on")
vim.cmd("colorscheme desert")
vim.opt.number     = true
vim.opt.whichwrap  = vim.opt.whichwrap + "<,>,h,l"

-- h / Left  — move to end of previous line when at column 1
vim.keymap.set("n", "h",     function()
  return vim.fn.col(".") == 1 and "k$" or "h"
end, { expr = true, silent = true })
vim.keymap.set("n", "<Left>", function()
  return vim.fn.col(".") == 1 and "k$" or "h"
end, { expr = true, silent = true })

-- l / Right — move to start of next line when at last column
vim.keymap.set("n", "l",      function()
  return vim.fn.col(".") == vim.fn.col("$") and "j0" or "l"
end, { expr = true, silent = true })
vim.keymap.set("n", "<Right>", function()
  return vim.fn.col(".") == vim.fn.col("$") and "j0" or "l"
end, { expr = true, silent = true })

-- ── 2. Proto filetype + syntax highlighting (spec 0145) ──────────────────────

vim.filetype.add({
  extension = {
    proto     = "proto",
    textproto = "textproto",
    pbtxt     = "textproto",
  },
})

vim.api.nvim_create_autocmd("FileType", {
  pattern  = "proto",
  callback = function(args)
    vim.bo[args.buf].commentstring = "// %s"
    vim.cmd([[
      syntax match  protoComment "//.*$"
      syntax region protoComment start="/\*" end="\*/"
      syntax region protoString  start=+"+ end=+"+
      syntax keyword protoKeyword syntax package import option message enum
            \ service rpc returns repeated optional required reserved oneof
            \ map extend extensions group stream
      highlight default link protoComment Comment
      highlight default link protoString  String
      highlight default link protoKeyword Keyword
    ]])

    -- ── 3. buf LSP client (spec 0145) ──────────────────────────────────────
    local root = vim.fs.root(args.buf, { "buf.yaml", "buf.work.yaml", ".git" })
        or vim.env.PROTOTEXT_PROTO_ROOT

    vim.lsp.start({
      name     = "buf",
      cmd      = { "buf", "lsp", "serve" },
      root_dir = root,
    })
  end,
})

vim.api.nvim_create_autocmd("FileType", {
  pattern  = "textproto",
  callback = function()
    vim.bo.commentstring = "# %s"
    vim.cmd([=[
      syntax match  tpComment   "#.*$"
      syntax region tpString    start=+"+ end=+"+ skip=+\\"+
      syntax region tpString    start=+'+ end=+'+ skip=+\\'+
      syntax match  tpNumber    "\<-\?\d\+\(\.\d*\)\?\([eE][+-]\?\d\+\)\?\>"
      syntax match  tpNumber    "\<0[xX][0-9a-fA-F]\+\>"
      syntax keyword tpBool     true false True False
      syntax match  tpField     "^\s*\zs[a-zA-Z_][a-zA-Z0-9_]*\ze\s*[:{<\[]"
      syntax match  tpFieldExt  "^\s*\zs\[[^\]]*\]\ze\s*[:{<\[]"
      syntax match  tpDelim     "[{}<>\[\]]"
      highlight default link tpComment  Comment
      highlight default link tpString   String
      highlight default link tpNumber   Number
      highlight default link tpBool     Boolean
      highlight default link tpField    Identifier
      highlight default link tpFieldExt PreProc
      highlight default link tpDelim    Delimiter
    ]=])
  end,
})

vim.api.nvim_create_autocmd("LspAttach", {
  callback = function(args)
    local opts = { buffer = args.buf, silent = true }
    vim.keymap.set("n", "gd", vim.lsp.buf.definition, opts)
    vim.keymap.set("n", "gr", vim.lsp.buf.references, opts)
    vim.keymap.set("n", "K",  vim.lsp.buf.hover,      opts)
  end,
})
