/* 两个窗口统一显示 IPC 失败，避免保存失败被误认为成功。 */
window.invokeWithFeedback = async (command, args) => {
  try {
    return await window.__TAURI__.core.invoke(command, args);
  } catch (error) {
    let message = document.getElementById("operationError");
    if (!message) {
      message = document.createElement("div");
      message.id = "operationError";
      message.setAttribute("role", "alert");
      message.style.cssText = "position:fixed;bottom:8px;left:8px;right:8px;z-index:9999;background:#fff0f0;color:#8b0000;padding:10px;border:1px solid #b00020;cursor:pointer";
      message.title = "点击关闭";
      message.addEventListener("click", () => message.remove());
      document.body.appendChild(message);
    }
    message.textContent = `操作失败：${String(error)}`;
    throw error;
  }
};
