return {
  {
    "mfussenegger/nvim-dap",
    dependencies = {
      "rcarriga/nvim-dap-ui",
      "nvim-neotest/nvim-nio",
    },
    config = function()
      local dap = require("dap")
      local dapui = require("dapui")

      dapui.setup()

      -- デバッグ開始時にUIを自動で開く
      dap.listeners.after.event_initialized["dapui_config"] = function()
        dapui.open()
      end
      dap.listeners.before.event_terminated["dapui_config"] = function()
        dapui.close()
      end
      dap.listeners.before.event_exited["dapui_config"] = function()
        dapui.close()
      end
    end,
    keys = {
      -- 基本操作
      {
        ";dc",
        function()
          require("dap").continue()
        end,
        desc = "Continue",
      },
      {
        ";dn",
        function()
          require("dap").step_over()
        end,
        desc = "Step Over (Next)",
      },
      {
        ";ds",
        function()
          require("dap").step_into()
        end,
        desc = "Step Into",
      },
      {
        ";do",
        function()
          require("dap").step_out()
        end,
        desc = "Step Out",
      },
      {
        ";dt",
        function()
          require("dap").terminate()
        end,
        desc = "Terminate",
      },

      -- ブレークポイント
      {
        ";db",
        function()
          require("dap").toggle_breakpoint()
        end,
        desc = "Toggle Breakpoint",
      },
      {
        ";dB",
        function()
          require("dap").set_breakpoint(vim.fn.input("Condition: "))
        end,
        desc = "Conditional Breakpoint",
      },

      -- UI/REPL
      {
        ";du",
        function()
          require("dapui").toggle()
        end,
        desc = "Toggle DAP UI",
      },
      {
        ";dr",
        function()
          require("dap").repl.open()
        end,
        desc = "Open REPL",
      },

      -- Rust専用（rustaceanvim連携）
      {
        ";dd",
        function()
          vim.cmd.RustLsp("debuggables")
        end,
        desc = "Rust Debuggables",
        ft = "rust",
      },
    },
  },
}
