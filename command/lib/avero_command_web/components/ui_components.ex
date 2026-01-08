defmodule AveroCommandWeb.UIComponents do
  @moduledoc """
  Reusable UI components with dark mode support.
  """
  use Phoenix.Component

  @doc """
  Renders a card container with dark mode support.
  """
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def card(assigns) do
    ~H"""
    <div class={[
      "rounded-xl border border-gray-200 bg-white p-4 sm:p-6",
      "dark:border-gray-800 dark:bg-gray-900",
      @class
    ]}>
      <%= render_slot(@inner_block) %>
    </div>
    """
  end

  @doc """
  Renders a badge/pill for status indicators.
  """
  attr :variant, :atom, default: :default, values: [:default, :success, :warning, :error, :info]
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def badge(assigns) do
    ~H"""
    <span class={[
      "inline-flex items-center rounded-full px-2.5 py-0.5 text-xs font-medium",
      badge_variant_class(@variant),
      @class
    ]}>
      <%= render_slot(@inner_block) %>
    </span>
    """
  end

  defp badge_variant_class(:default), do: "bg-gray-100 text-gray-800 dark:bg-gray-800 dark:text-gray-200"
  defp badge_variant_class(:success), do: "bg-green-100 text-green-800 dark:bg-green-900/30 dark:text-green-400"
  defp badge_variant_class(:warning), do: "bg-yellow-100 text-yellow-800 dark:bg-yellow-900/30 dark:text-yellow-400"
  defp badge_variant_class(:error), do: "bg-red-100 text-red-800 dark:bg-red-900/30 dark:text-red-400"
  defp badge_variant_class(:info), do: "bg-blue-100 text-blue-800 dark:bg-blue-900/30 dark:text-blue-400"

  @doc """
  Renders a page header with optional actions.
  """
  attr :title, :string, required: true
  attr :subtitle, :string, default: nil
  slot :actions

  def page_header(assigns) do
    ~H"""
    <div class="mb-6 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div>
        <h1 class="text-xl font-semibold text-gray-900 dark:text-white sm:text-2xl">
          <%= @title %>
        </h1>
        <p :if={@subtitle} class="mt-1 text-sm text-gray-500 dark:text-gray-400">
          <%= @subtitle %>
        </p>
      </div>
      <div :if={@actions != []} class="flex items-center gap-3">
        <%= render_slot(@actions) %>
      </div>
    </div>
    """
  end

  @doc """
  Renders a data table wrapper with dark mode.
  """
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def data_table(assigns) do
    ~H"""
    <div class={[
      "overflow-hidden rounded-xl border border-gray-200 dark:border-gray-800",
      @class
    ]}>
      <div class="overflow-x-auto">
        <table class="min-w-full divide-y divide-gray-200 dark:divide-gray-800">
          <%= render_slot(@inner_block) %>
        </table>
      </div>
    </div>
    """
  end

  @doc """
  Renders a table header row.
  """
  slot :inner_block, required: true

  def table_head(assigns) do
    ~H"""
    <thead class="bg-gray-50 dark:bg-gray-800/50">
      <tr>
        <%= render_slot(@inner_block) %>
      </tr>
    </thead>
    """
  end

  @doc """
  Renders a table header cell.
  """
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def th(assigns) do
    ~H"""
    <th class={[
      "px-4 py-3 text-left text-xs font-medium uppercase tracking-wider",
      "text-gray-500 dark:text-gray-400",
      @class
    ]}>
      <%= render_slot(@inner_block) %>
    </th>
    """
  end

  @doc """
  Renders a table body.
  """
  slot :inner_block, required: true

  def table_body(assigns) do
    ~H"""
    <tbody class="divide-y divide-gray-200 bg-white dark:divide-gray-800 dark:bg-gray-900">
      <%= render_slot(@inner_block) %>
    </tbody>
    """
  end

  @doc """
  Renders a table cell.
  """
  attr :class, :string, default: nil
  slot :inner_block, required: true

  def td(assigns) do
    ~H"""
    <td class={[
      "px-4 py-3 text-sm text-gray-700 dark:text-gray-300",
      @class
    ]}>
      <%= render_slot(@inner_block) %>
    </td>
    """
  end

  @doc """
  Renders a button with variants.
  """
  attr :type, :string, default: "button"
  attr :variant, :atom, default: :primary, values: [:primary, :secondary, :danger, :ghost]
  attr :size, :atom, default: :md, values: [:sm, :md, :lg]
  attr :class, :string, default: nil
  attr :rest, :global, include: ~w(disabled phx-click phx-target phx-value-id)
  slot :inner_block, required: true

  def button(assigns) do
    ~H"""
    <button
      type={@type}
      class={[
        "inline-flex items-center justify-center gap-2 rounded-lg font-medium",
        "transition-colors duration-200",
        "disabled:opacity-50 disabled:cursor-not-allowed",
        button_size_class(@size),
        button_variant_class(@variant),
        @class
      ]}
      {@rest}
    >
      <%= render_slot(@inner_block) %>
    </button>
    """
  end

  defp button_size_class(:sm), do: "px-3 py-1.5 text-sm"
  defp button_size_class(:md), do: "px-4 py-2 text-sm"
  defp button_size_class(:lg), do: "px-5 py-2.5 text-base"

  defp button_variant_class(:primary) do
    "bg-brand-500 text-white hover:bg-brand-600 dark:bg-brand-600 dark:hover:bg-brand-700"
  end
  defp button_variant_class(:secondary) do
    "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700"
  end
  defp button_variant_class(:danger) do
    "bg-red-500 text-white hover:bg-red-600 dark:bg-red-600 dark:hover:bg-red-700"
  end
  defp button_variant_class(:ghost) do
    "bg-transparent text-gray-700 hover:bg-gray-100 dark:text-gray-300 dark:hover:bg-gray-800"
  end

  @doc """
  Renders a text input field with dark mode.
  """
  attr :id, :string, required: true
  attr :name, :string, required: true
  attr :type, :string, default: "text"
  attr :value, :any, default: nil
  attr :placeholder, :string, default: nil
  attr :class, :string, default: nil
  attr :rest, :global, include: ~w(disabled readonly required autofocus phx-change phx-blur)

  def input(assigns) do
    ~H"""
    <input
      type={@type}
      id={@id}
      name={@name}
      value={@value}
      placeholder={@placeholder}
      class={[
        "block w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm",
        "text-gray-900 placeholder-gray-400",
        "focus:border-brand-500 focus:outline-none focus:ring-1 focus:ring-brand-500",
        "dark:border-gray-700 dark:bg-gray-800 dark:text-white dark:placeholder-gray-500",
        "dark:focus:border-brand-500",
        @class
      ]}
      {@rest}
    />
    """
  end

  @doc """
  Renders a select dropdown with dark mode.
  """
  attr :id, :string, required: true
  attr :name, :string, required: true
  attr :options, :list, required: true
  attr :value, :any, default: nil
  attr :prompt, :string, default: nil
  attr :class, :string, default: nil
  attr :rest, :global, include: ~w(disabled required phx-change)

  def select(assigns) do
    ~H"""
    <select
      id={@id}
      name={@name}
      class={[
        "block w-full rounded-lg border border-gray-300 bg-white px-3 py-2 text-sm",
        "text-gray-900",
        "focus:border-brand-500 focus:outline-none focus:ring-1 focus:ring-brand-500",
        "dark:border-gray-700 dark:bg-gray-800 dark:text-white",
        @class
      ]}
      {@rest}
    >
      <option :if={@prompt} value=""><%= @prompt %></option>
      <%= for {label, val} <- @options do %>
        <option value={val} selected={to_string(val) == to_string(@value)}><%= label %></option>
      <% end %>
    </select>
    """
  end

  @doc """
  Renders an empty state placeholder.
  """
  attr :title, :string, required: true
  attr :description, :string, default: nil
  slot :icon
  slot :actions

  def empty_state(assigns) do
    ~H"""
    <div class="flex flex-col items-center justify-center py-12 text-center">
      <div :if={@icon != []} class="mb-4 text-gray-400 dark:text-gray-600">
        <%= render_slot(@icon) %>
      </div>
      <h3 class="text-lg font-medium text-gray-900 dark:text-white"><%= @title %></h3>
      <p :if={@description} class="mt-1 text-sm text-gray-500 dark:text-gray-400">
        <%= @description %>
      </p>
      <div :if={@actions != []} class="mt-4">
        <%= render_slot(@actions) %>
      </div>
    </div>
    """
  end
end
