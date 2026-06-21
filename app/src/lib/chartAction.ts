import { Chart, type ChartConfiguration } from "chart.js/auto";

/// Svelte action that renders a Chart.js chart on a <canvas> and keeps it in
/// sync with the provided configuration.
export function chartjs(node: HTMLCanvasElement, config: ChartConfiguration) {
  let chart = new Chart(node, config);
  return {
    update(newConfig: ChartConfiguration) {
      chart.destroy();
      chart = new Chart(node, newConfig);
    },
    destroy() {
      chart.destroy();
    },
  };
}
