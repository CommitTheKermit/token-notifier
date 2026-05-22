/* Local lightweight Chart-compatible renderer for Token Notifier's single line chart. */
(function (global) {
  class Chart {
    constructor(canvas, config) {
      this.canvas = canvas;
      this.ctx = canvas.getContext('2d');
      this.config = config;
      this.draw();
    }

    destroy() {
      this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);
    }

    draw() {
      const ctx = this.ctx;
      const { width, height } = this.canvas;
      const datasets = this.config.data.datasets || [];
      const labels = this.config.data.labels || [];
      ctx.clearRect(0, 0, width, height);
      ctx.lineWidth = 1;
      ctx.strokeStyle = 'rgba(128,128,128,.25)';
      for (let i = 0; i < 4; i += 1) {
        const y = 24 + ((height - 48) * i) / 3;
        ctx.beginPath();
        ctx.moveTo(36, y);
        ctx.lineTo(width - 12, y);
        ctx.stroke();
      }
      const max = Math.max(1, ...datasets.flatMap((dataset) => dataset.data));
      datasets.forEach((dataset) => {
        ctx.strokeStyle = dataset.borderColor || '#4f8cff';
        ctx.lineWidth = 2;
        ctx.beginPath();
        dataset.data.forEach((value, index) => {
          const x = 36 + ((width - 54) * index) / Math.max(1, labels.length - 1);
          const y = height - 24 - ((height - 52) * value) / max;
          if (index === 0) ctx.moveTo(x, y);
          else ctx.lineTo(x, y);
        });
        ctx.stroke();
      });
    }
  }
  global.Chart = Chart;
})(window);
