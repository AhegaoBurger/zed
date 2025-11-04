import marimo

__generated_with = "0.9.0"
app = marimo.App(width="medium")


@app.cell
def __():
    import numpy as np
    import matplotlib.pyplot as plt
    return np, plt


@app.cell
def __(np):
    # Generate some data
    x = np.linspace(0, 10, 100)
    y = np.sin(x)
    return x, y


@app.cell
def __(plt, x, y):
    # Plot the data
    plt.figure(figsize=(10, 6))
    plt.plot(x, y)
    plt.title("Sine Wave")
    plt.xlabel("X")
    plt.ylabel("Y")
    plt.grid(True)
    return


if __name__ == "__main__":
    app.run()
