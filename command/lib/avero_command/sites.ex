defmodule AveroCommand.Sites do
  @moduledoc """
  Site configuration for multi-site support.

  Each site has specific configuration for:
  - Gateway IP addresses (Tailscale)
  - Grafana dashboard URLs
  - POS zone definitions
  - Display names
  """

  @sites %{
    "netto" => %{
      id: "AP-NETTO-GR-01",
      name: "Netto",
      gateway_ip: "100.80.187.3",
      grafana_dashboard: "command-live",
      grafana_site: "netto",
      pos_zones: ["POS_1", "POS_2", "POS_3", "POS_4", "POS_5"]
    },
    "avero" => %{
      id: "AP-AVERO-GR-01",
      name: "Avero HQ",
      gateway_ip: "100.65.110.63",
      grafana_dashboard: "command-live",
      grafana_site: "avero",
      pos_zones: ["POS_1"]
    }
  }

  @doc """
  Get all available sites as a list of {key, config} tuples.
  """
  def all do
    @sites
    |> Enum.map(fn {key, config} -> {key, config} end)
    |> Enum.sort_by(fn {key, _} -> key end)
  end

  @doc """
  Get all available site keys as a list of strings.
  """
  def keys do
    @sites
    |> Map.keys()
    |> Enum.sort()
  end

  @doc """
  Get site configuration by key (e.g., "netto", "avero").
  """
  def get(site_key) when is_binary(site_key) do
    Map.get(@sites, site_key)
  end

  def get(_), do: nil

  @doc """
  Get site configuration by site ID (e.g., "AP-NETTO-GR-01").
  """
  def get_by_id(site_id) when is_binary(site_id) do
    @sites
    |> Enum.find(fn {_key, config} -> config.id == site_id end)
    |> case do
      {key, config} -> {key, config}
      nil -> nil
    end
  end

  def get_by_id(_), do: nil

  @doc """
  Get the site key from a site ID.
  """
  def key_from_id(site_id) when is_binary(site_id) do
    case get_by_id(site_id) do
      {key, _config} -> key
      nil -> nil
    end
  end

  def key_from_id(_), do: nil

  @doc """
  Get the default site key.
  """
  def default_key, do: "netto"

  @doc """
  Get the default site ID.
  """
  def default_id, do: @sites["netto"].id

  @doc """
  Get the gateway URL for a site (for HTTP API calls).
  """
  def gateway_url(site_key, path \\ "") when is_binary(site_key) do
    case get(site_key) do
      %{gateway_ip: ip} -> "http://#{ip}:9090#{path}"
      nil -> nil
    end
  end

  @doc """
  Get the Grafana panel URL for a site.
  """
  def grafana_panel_url(site_key, panel_id, opts \\ []) when is_binary(site_key) do
    from = Keyword.get(opts, :from, "now-30m")
    to = Keyword.get(opts, :to, "now")
    refresh = Keyword.get(opts, :refresh, "30s")

    case get(site_key) do
      %{grafana_dashboard: dashboard, grafana_site: grafana_site} ->
        "https://grafana.avero.is/d-solo/#{dashboard}/#{dashboard}?orgId=1&panelId=#{panel_id}&theme=dark&from=#{from}&to=#{to}&refresh=#{refresh}&var-site=#{grafana_site}"

      nil ->
        nil
    end
  end

  @doc """
  Get the full Grafana dashboard URL for a site.
  """
  def grafana_dashboard_url(site_key) when is_binary(site_key) do
    case get(site_key) do
      %{grafana_dashboard: dashboard, grafana_site: grafana_site} ->
        "https://grafana.avero.is/d/#{dashboard}/#{dashboard}?orgId=1&theme=dark&kiosk=tv&refresh=5s&var-site=#{grafana_site}"

      nil ->
        nil
    end
  end

  @doc """
  Get POS zones for a site.
  """
  def pos_zones(site_key) when is_binary(site_key) do
    case get(site_key) do
      %{pos_zones: zones} -> zones
      nil -> []
    end
  end

  @doc """
  Check if a site key is valid.
  """
  def valid?(site_key) when is_binary(site_key), do: Map.has_key?(@sites, site_key)
  def valid?(_), do: false
end
