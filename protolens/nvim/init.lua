-- SPDX-FileCopyrightText: 2026 Frederic Ruget <fred@atlant.is> (GitHub: @douzebis)
--
-- SPDX-License-Identifier: MIT

-- ── Aesthetics ───────────────────────────────────────────────────────────────

vim.cmd("syntax on")
vim.cmd("colorscheme desert")
vim.opt.number    = true
vim.opt.whichwrap = vim.opt.whichwrap + "<,>,h,l"

-- h / Left  — move to end of previous line when at column 1
vim.keymap.set("n", "h", function()
  return vim.fn.col(".") == 1 and "k$" or "h"
end, { expr = true, silent = true })
vim.keymap.set("n", "<Left>", function()
  return vim.fn.col(".") == 1 and "k$" or "h"
end, { expr = true, silent = true })

-- l / Right — move to start of next line when at last column
vim.keymap.set("n", "l", function()
  return vim.fn.col(".") == vim.fn.col("$") and "j0" or "l"
end, { expr = true, silent = true })
vim.keymap.set("n", "<Right>", function()
  return vim.fn.col(".") == vim.fn.col("$") and "j0" or "l"
end, { expr = true, silent = true })

-- ── Proto filetype + syntax highlighting ─────────────────────────────────────

vim.filetype.add({ extension = { proto = "proto" } })

vim.api.nvim_create_autocmd("FileType", {
  pattern = "proto",
  callback = function(args)
    vim.bo[args.buf].commentstring = "// %s"
    vim.cmd([[
      syntax match protoComment "//.*$"
      syntax region protoComment start="/\*" end="\*/"
      syntax region protoString start=+"+ end=+"+
      syntax keyword protoKeyword syntax package import option message enum
            \ service rpc returns repeated optional required reserved oneof
            \ map extend extensions group stream
      highlight default link protoComment Comment
      highlight default link protoString String
      highlight default link protoKeyword Keyword
    ]])

    local root = vim.fs.root(args.buf, { "buf.yaml", "buf.work.yaml", ".git" })
        or vim.env.PROTOTEXT_PROTO_ROOT

    vim.lsp.start({
      name = "buf",
      cmd = { "buf", "lsp", "serve" },
      root_dir = root,
    })
  end,
})

vim.api.nvim_create_autocmd("LspAttach", {
  callback = function(args)
    local opts = { buffer = args.buf, silent = true }
    vim.keymap.set("n", "gd", vim.lsp.buf.definition, opts)
    vim.keymap.set("n", "gr", vim.lsp.buf.references, opts)
    vim.keymap.set("n", "K", vim.lsp.buf.hover, opts)
  end,
})
