local status, signature = pcall(require, 'lsp_signature')
if (not status) then return end

local signature_setup = signature.setup()

local deno_setup = {
	on_attach = function(client, bufnr)
		signature.on_attach(signature_setup, bufnr)
	end
}

-- local lua_setup = {
-- 	on_attach = function(client, bufnr)
-- 		signature.on_attach({
-- 			bind = true,
-- 			handlers_opts = {
-- 				border = "rounded"
-- 			}
-- 		}, bufnr)
-- 	end
-- }
--
-- local current_signature = function(width)
--   if not pcall(require, 'lsp_signature') then return end
--   local sig = require("lsp_signature").status_line(width)
--   return sig.label .. "🐼" .. sig.hint
-- end

require('lspconfig').denols.setup(deno_setup)
-- require('lspconfig').sumneko_lua.setup(lua_setup)
