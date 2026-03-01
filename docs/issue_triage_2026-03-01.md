# OpenFang 可执行修复任务筛选（基于当前 open issues）

更新时间：2026-03-01
数据源：`https://api.github.com/repos/RightNow-AI/openfang/issues?state=open&per_page=100`

## 建议优先处理（可在本仓库内直接修复）

1. **#106 – PATCH /api/agents/:id does not update model or name fields**  
   - 类型：后端 API 兼容性 / 回归问题  
   - 可做原因：`routes.rs` 中现有更新接口主要是 `PUT /api/agents/:id` 与 `PATCH /api/agents/{id}/config`，很可能是旧路径与新路径行为不一致导致。补齐兼容路由或统一字段更新逻辑即可。  
   - 预计工作量：**S-M**

2. **#113 – Hand settings API returns empty 200 responses**  
   - 类型：后端 API 返回体 bug  
   - 可做原因：`/api/hands` 系列路由已存在，若 settings 相关 handler 返回 `()` 或未序列化 JSON，容易出现 `200 + empty body`。定位并修复响应结构即可。  
   - 预计工作量：**M**

3. **#112 – openfang skill search returns 422**  
   - 类型：CLI / Marketplace 集成 bug  
   - 可做原因：`openfang-skills` 内同时存在 `marketplace` 与 `clawhub` 客户端实现，422 常见于请求参数或 endpoint 不匹配；可通过调整 search 请求构造与容错解析修复。  
   - 预计工作量：**S-M**

4. **#108 – model ID with provider prefix causes OpenRouter 400**  
   - 类型：模型标识规范化 bug  
   - 可做原因：通常在 provider adapter 层做 model id normalize（如去 `openrouter/` 前缀）即可；影响面集中、可测试性强。  
   - 预计工作量：**S**

5. **#107 – migrate from OpenClaw corrupts OPENROUTER_API_KEY**  
   - 类型：迁移工具 bug  
   - 可做原因：仓库有独立 `openfang-migrate` crate；该问题看起来是字符串拼接/解析逻辑错误，容易添加回归测试。  
   - 预计工作量：**S**

6. **#104 – UTF-8 byte boundary panic in truncation**  
   - 类型：稳定性 bug（panic）  
   - 可做原因：可改为基于 `char_indices` / `unicode-segmentation` 安全截断，属于明确可复现的健壮性修复。  
   - 预计工作量：**S-M**

7. **#117 – Web UI base_url 保存按钮灰掉 + 重启不加载配置**  
   - 类型：前后端联调 bug（Windows）  
   - 可做原因：现象明确，通常是前端 dirty-check 条件或后端配置持久化字段遗漏；可在本仓库同时排查。  
   - 预计工作量：**M**

## 暂不建议优先（更依赖平台/发布流程）

1. **#109 / #97 / #44（macOS / Linux 二进制或运行时挂起）**  
   - 偏向构建链、发布产物、系统动态库、签名或平台特性问题，短期修复依赖 CI/CD 与多平台验证。

2. **#111（WhatsApp gateway 启动问题）**  
   - 可能涉及 Node/npm 运行环境、外部依赖或单独包发布，排障链路更长。

3. **#120（`openfang hand` 子命令未识别）**  
   - 更像是命令设计变更/文档不一致，不一定是代码缺陷；需要先确认预期 CLI 语义。

## 推荐开工顺序

1. #108（快修 + 高确定性）  
2. #107（快修 + 可补测试）  
3. #106（用户面影响大）  
4. #112（CLI 高频命令）  
5. #104（稳定性）  
6. #113（API 可用性）  
7. #117（跨端联调）
